use std::sync::Arc;
use std::{i32, path::PathBuf, str::FromStr};

use anyhow::Result;
use async_ffmpeg_sidecar::command::FfmpegCommand;
use futures::future::join_all;
use futures_util::StreamExt;
use speedate::Time;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;
use crate::ffmpeg::{replaygain_analysis, transcode_command, write_replay_gain, FfprobeOutput};
use crate::utils::{create_client, fetch_rss_channel, sanitize_filename};
use async_tempfile::TempFile;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize, Default)]
pub struct DownloadOptions {
    pub override_download_limit: Option<Option<usize>>,
    pub compress: bool,
    pub download_listened: bool,
    pub base_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum DownloadMessages {
    StartDownload {
        uuid: Uuid,
        total_size: usize,
        title: String,
        author: String
    },
    DownloadProgress {
        uuid: Uuid,
        position: usize
    },
    FlushingDownload {
        uuid: Uuid
    },
    AnalyzingMedia {
        uuid: Uuid
    },
    StartTranscodeAndCompress {
        uuid: Uuid,
        compressing: bool,
        length_seconds: f64,
    },
    TranscodeAndCompressProgress {
        uuid: Uuid,
        position: f64,
    },
    Finish {
        uuid: Uuid
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct PodderDB {
    #[serde(default)]
    pub podcasts: Vec<Podcast>,
    #[serde(default)]
    pub youtube_playlists: Vec<YoutubePlaylist>,
    #[serde(default)]
    pub compress: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Podcast {
    pub title: String,
    pub description: Option<String>,
    pub xml_url: Url,
    pub html_url: Option<Url>,
    pub auto_download_limit: Option<usize>,
    pub delete_old_episodes: bool,
    pub episodes: Vec<Episode>,
    pub last_refreshed: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Episode {
    pub guid: String,
    pub title: String,
    pub podcast: String,
    pub enclosure: Enclosure,
    pub pub_date: DateTime<Utc>,
    pub file_path: Option<PathBuf>,
    pub listened_to: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Enclosure {
    pub url: String,
    pub length: i32,
    pub mime_type: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct YoutubePlaylist {
    pub title: String,
    pub url: String,
    pub auto_download_limit: Option<usize>,
    pub delete_old_episodes: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct YoutubeVideo {
    pub title: String,
    pub channel: String,
    pub id: String,
    pub file_path: Option<PathBuf>,
    pub listened_to: bool,
}

pub struct DownloadQueueElement<T: Content + Sync> {
    pub content: Arc<T>,
    pub tempfile: TempFile,
    pub uuid: Uuid,
}

pub struct UpdateResults {
    pub collection: String,
    pub new_content: Vec<String>,
}

pub trait Library: Sync + Send {
    type Collection: Collection;
    type Content: Content;

    fn filename() -> &'static str;
    async fn add_from_url(&mut self, url: Url) -> Result<(String, usize)>;
    async fn update_all(&mut self, base_path: &PathBuf) -> Vec<UpdateResults>;
    async fn get_all_download_queue(&mut self, opts: &DownloadOptions) -> Vec<DownloadQueueElement<Self::Content>>;
}

pub trait Collection<T: WithSafeFilename = Self>:  WithSafeFilename + Sync + Send {
    type Content: Content;
    type Library: Library;
    type Collection: Collection;

    async fn update(&mut self, base_path: &PathBuf) -> Result<UpdateResults>;
    async fn get_download_queue(&mut self, opts: &DownloadOptions) -> Vec<DownloadQueueElement<Self::Content>>;
}

pub trait Content<T: WithSafeFilename = Self>: WithSafeFilename + Sync + Send {
    fn get_path(&self) -> Option<&PathBuf>;
    async fn download(&mut self, tf: &mut TempFile, opts: &DownloadOptions, uuid: Uuid, ch: flume::Sender<DownloadMessages>) -> Result<()>;
}

pub trait WithSafeFilename {
    fn safe_filename(&self) -> String;
}


impl Library for Vec<Podcast> {
    type Collection = Podcast;
    type Content = Episode;

    fn filename() -> &'static str { "podcasts" }


    async fn add_from_url(&mut self, url: Url) -> Result<(String, usize)> {
        if self.iter().any(|p| p.xml_url == url) {
            return Err(anyhow::anyhow!("Podcast with URL '{}' already exists", url));
        }

        let channel = fetch_rss_channel(&url).await?;
        let mut podcast = Podcast {
            title: channel.title,
            description: Some(channel.description),
            xml_url: url,
            html_url: Url::parse(&channel.link).ok(),
            auto_download_limit: Some(5),
            episodes: Vec::new(),
            last_refreshed: Utc::now(),
            delete_old_episodes: false,
        };

        for item in channel.items {
            if let Some(episode) = Episode::from_rss_item(item, podcast.title.clone()) {
                podcast.episodes.push(episode);
            }
        }

        podcast.episodes.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));

        let ret = (podcast.title.clone(), podcast.episodes.len());

        self.push(podcast);

        Ok(ret)
    }



    async fn update_all(&mut self, base_path: &PathBuf) -> Vec<UpdateResults> {
        let futures = self
            .iter_mut()
            .map(|p| async move { p.update(base_path).await.ok() });

        let results = join_all(futures).await;

        results.into_iter().filter_map(|r| r).collect()
    }

    async fn get_all_download_queue(&mut self, opts: &DownloadOptions) -> Vec<DownloadQueueElement<Self::Content>> {
        join_all(self.iter_mut().map(async |p| p.get_download_queue(opts).await)).await.into_iter().flatten().collect()
    }
}

impl Collection for Podcast {
    type Content = Episode;
    type Library = Vec<Podcast>;
    type Collection = Podcast;



    async fn update(&mut self, base_path: &PathBuf) -> Result<UpdateResults> {
        for episode in self
            .episodes
            .iter_mut()
            {
                if episode.file_path.as_ref().is_some_and(|p| base_path.join(p).exists()) {
                    episode.listened_to = true;
                    episode.file_path = None;
                }
            }

        let channel = fetch_rss_channel(&self.xml_url).await?;

        let mut updated_items: Vec<String> = Vec::new();
        for item in channel.items {
            let guid = item
                .guid
                .as_ref()
                .map(|g| g.value.clone())
                .unwrap_or_else(|| item.title.clone().unwrap_or_default());

            if !self.episodes.iter().any(|e| e.guid == guid) {
                if let Some(episode) = Episode::from_rss_item(item, self.title.clone()) {
                    updated_items.push(episode.title.clone());
                    self.episodes.push(episode);
                }
            }
        }

        Ok(UpdateResults { collection: self.title.clone(), new_content: updated_items })
    }



    async fn get_download_queue(&mut self, opts: &DownloadOptions) -> Vec<DownloadQueueElement<Self::Content>> {
        self.episodes.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));
        let file_name = self.safe_filename();
        join_all(self.episodes.iter_mut()
            .filter_map(|e| if e.file_path.is_none() && (opts.download_listened || !e.listened_to) {Some(e)} else {None})
            .take(opts.override_download_limit.as_ref().unwrap_or(&self.auto_download_limit).unwrap_or(usize::MAX))
            .map(async |e| {
                e.file_path = Some(PathBuf::from_str(Self::Library::filename()).unwrap()
                    .join(PathBuf::from_str(&file_name).unwrap())
                    .join(PathBuf::from_str(&e.safe_filename()).unwrap()));
                DownloadQueueElement {
                    content: Arc::new(e.clone()),
                    tempfile: TempFile::new().await.unwrap(),
                    uuid: Uuid::new_v4()
                }
            }))
            .await
            .into_iter()
            .collect()
    }
}

impl Content for Episode {
    fn get_path(&self) -> Option<&PathBuf> {
        self.file_path.as_ref()
    }

    async fn download(&mut self, tf: &mut TempFile, opts: &DownloadOptions, uuid: Uuid, tx: flume::Sender<DownloadMessages>) -> Result<()> {
        let response = create_client().get(&self.enclosure.url).send().await?;
        let total_size = response.content_length().unwrap_or(0);
        let _ = tx.send(DownloadMessages::StartDownload { uuid, total_size: total_size as usize, title: self.title.clone(), author: self.podcast.clone() });
        let mut downloaded = 0usize;
        let mut count = 0;

        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            tf.write_all(&chunk).await?;
            count += 1;
            downloaded += chunk.len();
            if count % 10 == 0 {
                let _ = tx.try_send(DownloadMessages::DownloadProgress { uuid, position: downloaded });
            }
        }
        let _ = tx.send(DownloadMessages::FlushingDownload { uuid });
        tf.flush().await;
        let _ = tx.send(DownloadMessages::AnalyzingMedia { uuid });

        let (track_gain, track_peak) = replaygain_analysis(tf.file_path()).await?;
        let ffprobe_output = FfprobeOutput::from_file(tf.file_path().to_str().unwrap()).await?;

        tx.send(DownloadMessages::StartTranscodeAndCompress { uuid, compressing: opts.compress, length_seconds: ffprobe_output.duration_seconds().unwrap_or_default() });

        let output_path = opts.base_path.join(self.file_path.as_ref().unwrap());

        let cmd = if opts.compress {
            transcode_command(tf.file_path(), &output_path, crate::ffmpeg::TranscodeOptions::Compress)
        } else {
            transcode_command(tf.file_path(), &output_path, crate::ffmpeg::TranscodeOptions::Default)
        };

        ffmpeg_channel_reporter(cmd, uuid, &tx).await?;

        write_replay_gain(&output_path, track_gain, track_peak)?;
        tx.send(DownloadMessages::Finish { uuid });

        Ok(())

    }
}

async fn ffmpeg_channel_reporter(mut cmd: FfmpegCommand, uuid: Uuid, tx: &flume::Sender<DownloadMessages>) -> Result<()> {
    let mut stream = cmd.spawn()?.stream()?;
    while let Some(event) = stream.next().await {
        match event {
            async_ffmpeg_sidecar::event::FfmpegEvent::Progress(p) => {
                let t = Time::parse_str(&p.time).unwrap();
                let seconds = t.hour as f64 * 3600.0
                    + t.minute as f64 * 60.0
                    + t.second as f64
                    + t.microsecond as f64 / 1_000_000.0;
                let _ = tx.try_send(DownloadMessages::TranscodeAndCompressProgress { uuid, position: seconds });
            },
            _ => {}
        }
    }
    Ok(())
}

impl WithSafeFilename for Episode {
    fn safe_filename(&self) -> String {
        format!("{}.mp3", sanitize_filename(&self.title))
    }
}

impl WithSafeFilename for Podcast {
    fn safe_filename(&self) -> String {
        sanitize_filename(&self.title)
    }
}


impl Episode {
    fn from_rss_item(item: rss::Item, podcast_title: String) -> Option<Episode> {
        let enclosure = item.enclosure?;
        let guid = item
            .guid
            .map(|g| g.value)
            .unwrap_or_else(|| item.title.clone().unwrap_or_default());

        Some(Episode {
            guid,
            title: item.title.unwrap_or_default(),
            podcast: podcast_title,
            enclosure: Enclosure {
                url: enclosure.url,
                length: enclosure.length.parse().unwrap_or(0),
                mime_type: enclosure.mime_type,
            },
            pub_date: item
                .pub_date
                .and_then(|d| DateTime::parse_from_rfc2822(&d).ok())
                .map(|d| d.into())
                .unwrap_or_else(Utc::now),
            file_path: None,
            listened_to: false,
        })
    }
}
