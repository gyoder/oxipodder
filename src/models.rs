use std::u32;
use std::{path::PathBuf, str::FromStr};
use anyhow::Result;
use async_ffmpeg_sidecar::command::FfmpegCommand;
use async_ffmpeg_sidecar::event::FfmpegEvent;
use futures_util::StreamExt;
use speedate::Time;
use tokio::fs::{create_dir_all, remove_file};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use crate::db;
use crate::ffmpeg::{replaygain_analysis, transcode_command, write_replay_gain, FfprobeOutput};
use crate::models::settings::get_settings;
use crate::utils::{create_client, fetch_rss_channel, sanitize_filename};
use async_tempfile::TempFile;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use sea_orm::{
    prelude::*, ActiveModelTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set
};
use tokio::sync::mpsc;

pub mod settings {
    use sea_orm::IntoActiveModel;

    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "settings")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub compress: bool,
        pub default_episode_limit_podcast: Option<u32>,
        pub default_episode_limit_youtube: Option<u32>,
        pub download_threads: u8,
    }

    #[derive(Copy, Clone, Debug, EnumIter)]
    pub enum Relation {}

    impl RelationTrait for Relation {
        fn def(&self) -> RelationDef {
            panic!("no relation")
        }
    }

    impl ActiveModelBehavior for ActiveModel {}

    pub async fn get_settings() -> Model {
        match Entity::find_by_id(1)
        .one(db())
        .await {
            Ok(Some(s)) => s,
            Err(_) | Ok(None) => {
                let s = Model {
                    id: 1,
                    compress: false,
                    default_episode_limit_podcast: Some(3),
                    default_episode_limit_youtube: None,
                    download_threads: 8,
                };
                s.into_active_model().insert(db()).await.unwrap()

            },
        }
    }
}


pub mod podcast {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "podcasts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub xml_url: String,
        pub title: String,
        pub description: Option<String>,
        pub html_url: Option<String>,
        pub auto_download_limit: Option<u32>,
        pub delete_old_episodes: bool,
        pub last_refreshed: DateTime<Utc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(has_many = "super::episode::Entity")]
        Episodes,
    }

    impl Related<super::episode::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Episodes.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod episode {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "episodes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub guid: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub podcast_xml_url: String,
        pub title: String,
        pub podcast_title: String,
        pub enclosure_url: String,
        pub enclosure_length: i32,
        pub enclosure_mime_type: String,
        pub pub_date: DateTime<Utc>,
        pub file_path: Option<String>,
        pub listened_to: bool,
        pub no_auto_delete: bool
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(
            belongs_to = "super::podcast::Entity",
            from = "Column::PodcastXmlUrl",
            to = "super::podcast::Column::XmlUrl"
        )]
        Podcast,
    }

    impl Related<super::podcast::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Podcast.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod youtube_playlist {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "youtube_playlists")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub url: String,
        pub title: String,
        pub auto_download_limit: Option<u32>,
        pub delete_old_episodes: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(has_many = "super::youtube_video::Entity")]
        Videos,
    }

    impl Related<super::youtube_video::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Videos.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod youtube_video {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "youtube_videos")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub playlist_url: String,
        pub title: String,
        pub channel: String,
        pub file_path: Option<String>,
        pub listened_to: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {
        #[sea_orm(
            belongs_to = "super::youtube_playlist::Entity",
            from = "Column::PlaylistUrl",
            to = "super::youtube_playlist::Column::Url"
        )]
        Playlist,
    }

    impl Related<super::youtube_playlist::Entity> for Entity {
        fn to() -> RelationDef {
            Relation::Playlist.def()
        }
    }

    impl ActiveModelBehavior for ActiveModel {}
}


#[derive(Clone, Serialize, Deserialize, Default)]
pub struct DownloadOptions {
    pub override_download_limit: Option<Option<usize>>,
    pub compress: bool,
    pub download_listened: bool,
    pub no_auto_delete: bool,
}

#[derive(Debug, Clone)]
pub enum DownloadMessages {
    StartDownload {
        uuid: Uuid,
        total_size: usize,
        title: String,
        author: String,
    },
    DownloadProgress {
        uuid: Uuid,
        position: usize,
    },
    FlushingDownload {
        uuid: Uuid,
    },
    AnalyzingMedia {
        uuid: Uuid,
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
        uuid: Uuid,
    },
}


#[derive(Clone, Debug, PartialEq)]
pub struct PodcastKey {
    pub xml_url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeKey {
    pub guid: String,
    pub podcast_xml_url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YoutubePlaylistKey {
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct YoutubeVideoKey {
    pub id: String,
    pub playlist_url: String,
}

pub struct DownloadQueueElement<T: Sync + Send> {
    pub content_key: T,
    pub uuid: Uuid,
}

pub struct UpdateResults {
    pub collection: String,
    pub new_content: Vec<String>,
}


pub trait Library: Sync + Send {
    type Collection: Collection;
    type Content: Content;
    type ContentKey: Clone + Send + Sync;

    fn filename() -> &'static str;
    async fn get_all() -> Vec<Box<Self::Collection>>;
    async fn add_from_url(url: Url) -> Result<(String, usize)>;
    async fn update_all() -> Vec<UpdateResults>;
    async fn get_all_download_queue(opts: &DownloadOptions) -> Vec<DownloadQueueElement<Self::ContentKey>>;
}

pub trait Collection<T: WithSafeFilename = Self>: WithSafeFilename + Sync + Send {
    type Content: Content;
    type Library: Library;
    type Collection: Collection;
    type ContentKey: Clone + Send + Sync;

    async fn update(&self) -> Result<UpdateResults>;
    async fn get_all_content(&self) -> Vec<Self::Content>;
    async fn get_download_queue(&self, opts: &DownloadOptions) -> Vec<DownloadQueueElement<Self::ContentKey>>;
}

pub trait Content<T: WithSafeFilename = Self>: WithSafeFilename + Sync + Send {
    type Key: Clone + Send + Sync;

    async fn get_by_key(key: &Self::Key) -> Result<Option<Self>> where Self: Sized;
    async fn get_path_by_key(key: &Self::Key) -> Result<Option<PathBuf>>;
    fn download_by_key(
        key: &Self::Key,
        opts: &DownloadOptions,
        uuid: Uuid,
        ch: mpsc::Sender<DownloadMessages>,
    ) -> impl std::future::Future<Output = Result<()>> + std::marker::Send;
}

pub trait WithSafeFilename {
    fn safe_filename(&self) -> String;
}

pub trait ContentDownloader {
    type Key: Clone + Send + Sync + 'static;
    type Content: Content<Key = Self::Key> + Send + Sync + 'static;
}

impl ContentDownloader for episode::Model {
    type Key = EpisodeKey;
    type Content = episode::Model;
}


pub struct PodcastLibrary;

impl Library for PodcastLibrary {
    type Collection = podcast::Model;
    type Content = episode::Model;
    type ContentKey = EpisodeKey;

    fn filename() -> &'static str {
        "podcasts"
    }

    async fn get_all() -> Vec<Box<Self::Collection>> {
        podcast::Entity::find().order_by_asc(podcast::Column::Title).all(db()).await.unwrap_or_default().into_iter().map(Box::new).collect()
    }

    async fn add_from_url(url: Url) -> Result<(String, usize)> {
        let xml_url = url.to_string();

        let existing = podcast::Entity::find_by_id(&xml_url).one(db()).await?;
        if existing.is_some() {
            return Err(anyhow::anyhow!("Podcast with URL '{}' already exists", url));
        }

        let channel = fetch_rss_channel(&url).await?;

        let s = get_settings().await;

        let podcast_model = podcast::ActiveModel {
            xml_url: Set(xml_url.clone()),
            title: Set(channel.title.clone()),
            description: Set(Some(channel.description)),
            html_url: Set(Url::parse(&channel.link).ok().map(|u| u.to_string())),
            auto_download_limit: Set(s.default_episode_limit_podcast),
            delete_old_episodes: Set(false),
            last_refreshed: Set(Utc::now()),
        };

        podcast_model.insert(db()).await?;


        let mut episode_count = 0;
        for item in channel.items {
            if let Some(episode_data) = Self::episode_from_rss_item(item, channel.title.clone(), xml_url.clone()) {
                episode_data.insert(db()).await?;
                episode_count += 1;
            }
        }

        Ok((channel.title, episode_count))
    }

    async fn update_all() -> Vec<UpdateResults> {
        let podcasts = podcast::Entity::find().all(db()).await.unwrap_or_default();
        let mut results = Vec::new();

        for podcast_model in podcasts {
            if let Ok(update_result) = podcast_model.update().await {
                results.push(update_result);
            }
        }

        results
    }

    async fn get_all_download_queue(opts: &DownloadOptions) -> Vec<DownloadQueueElement<Self::ContentKey>> {
        let podcasts = podcast::Entity::find().all(db()).await.unwrap_or_default();
        let mut queue = Vec::new();

        for podcast_model in podcasts {
            let mut podcast_queue = podcast_model.get_download_queue(opts).await;
            queue.append(&mut podcast_queue);
        }

        queue
    }
}

impl PodcastLibrary {
    fn episode_from_rss_item(item: rss::Item, podcast_title: String, podcast_xml_url: String) -> Option<episode::ActiveModel> {
        let enclosure = item.enclosure?;
        let guid = item
            .guid
            .map(|g| g.value)
            .unwrap_or_else(|| item.title.clone().unwrap_or_default());

        Some(episode::ActiveModel {
            guid: Set(guid),
            podcast_xml_url: Set(podcast_xml_url),
            title: Set(item.title.unwrap_or_default()),
            podcast_title: Set(podcast_title),
            enclosure_url: Set(enclosure.url),
            enclosure_length: Set(enclosure.length.parse().unwrap_or(0)),
            enclosure_mime_type: Set(enclosure.mime_type),
            pub_date: Set(item
                .pub_date
                .and_then(|d| DateTime::parse_from_rfc2822(&d).ok())
                .map(|d| d.into())
                .unwrap_or_else(Utc::now)),
            file_path: Set(None),
            listened_to: Set(false),
            no_auto_delete: Set(false)
        })
    }
}


impl Collection for podcast::Model {
    type Content = episode::Model;
    type Library = PodcastLibrary;
    type Collection = podcast::Model;
    type ContentKey = EpisodeKey;

    async fn update(&self) -> Result<UpdateResults> {

        let episodes = episode::Entity::find()
            .filter(episode::Column::PodcastXmlUrl.eq(&self.xml_url))
            .all(db())
            .await?;

        for episode in episodes {
            if let Some(ref file_path) = episode.file_path {
                if !PathBuf::from_str(file_path)?.exists() {
                    let mut episode_active: episode::ActiveModel = episode.into();
                    episode_active.listened_to = Set(true);
                    episode_active.file_path = Set(None);
                    episode_active.update(db()).await?;
                }
            }
        }


        let url = Url::parse(&self.xml_url)?;
        let channel = fetch_rss_channel(&url).await?;

        let mut updated_items: Vec<String> = Vec::new();
        for item in channel.items {
            let guid = item
                .guid
                .as_ref()
                .map(|g| g.value.clone())
                .unwrap_or_else(|| item.title.clone().unwrap_or_default());


            let existing = episode::Entity::find()
                .filter(episode::Column::Guid.eq(&guid))
                .filter(episode::Column::PodcastXmlUrl.eq(&self.xml_url))
                .one(db())
                .await?;

            if existing.is_none() {
                if let Some(episode_data) = PodcastLibrary::episode_from_rss_item(
                    item,
                    self.title.clone(),
                    self.xml_url.clone(),
                ) {
                    updated_items.push(episode_data.title.as_ref().clone());
                    episode_data.insert(db()).await?;
                }
            }
        }


        let mut podcast_active: podcast::ActiveModel = self.clone().into();
        podcast_active.last_refreshed = Set(Utc::now());
        podcast_active.update(db()).await?;

        if self.delete_old_episodes {
            let downloaded = episode::Entity::find()
                .filter(episode::Column::PodcastXmlUrl.eq(&self.xml_url))
                .filter(episode::Column::FilePath.is_not_null())
                .order_by_desc(episode::Column::PubDate)
                .all(db())
                .await?;

            let items: Vec<episode::ActiveModel> = downloaded.into_iter().skip(self.auto_download_limit.unwrap_or(u32::MAX) as usize)
                .filter_map(|e| {
                    if e.file_path.is_some() && !e.no_auto_delete {
                        Some(e.into())
                    } else {
                        None
                    }
                }).collect();

            for mut i in items {
                if let Set(Some(fp)) = i.file_path {
                    remove_file(PathBuf::from_str(&fp).unwrap()).await?
                }
                i.file_path = Set(None);
                i.update(db()).await?;
            }

        }

        Ok(UpdateResults {
            collection: self.title.clone(),
            new_content: updated_items,
        })
    }

    async fn get_all_content(&self) -> Vec<Self::Content> {
        episode::Entity::find().filter(episode::Column::PodcastXmlUrl.eq(&self.xml_url)).order_by_desc(episode::Column::PubDate).all(db()).await.unwrap_or_default()
    }

    async fn get_download_queue(&self, opts: &DownloadOptions) -> Vec<DownloadQueueElement<Self::ContentKey>> {
        let limit = opts
            .override_download_limit
            .as_ref()
            .unwrap_or(&self.auto_download_limit.map(|x| x as usize))
            .unwrap_or(usize::MAX);


        let episodes = episode::Entity::find()
            .filter(episode::Column::PodcastXmlUrl.eq(&self.xml_url))
            .order_by_desc(episode::Column::PubDate)
            .limit(limit as u64)
            .all(db())
            .await
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .filter(|e| e.file_path.is_none() && (!e.listened_to || opts.download_listened));

        let file_name = self.safe_filename();
        let mut queue = Vec::new();

        for episode in episodes {

            let file_path = PathBuf::from_str(PodcastLibrary::filename())
                .unwrap()
                .join(PathBuf::from_str(&file_name).unwrap())
                .join(PathBuf::from_str(&episode.safe_filename()).unwrap());

            let mut episode_active: episode::ActiveModel = episode.clone().into();
            episode_active.file_path = Set(Some(file_path.to_string_lossy().to_string()));

            if episode_active.update(db()).await.is_ok() {
                queue.push(DownloadQueueElement {
                    content_key: EpisodeKey {
                        guid: episode.guid,
                        podcast_xml_url: episode.podcast_xml_url,
                    },
                    uuid: Uuid::new_v4(),
                });
            }
        }

        queue
    }
}


impl Content for episode::Model {
    type Key = EpisodeKey;

    async fn get_by_key(key: &Self::Key) -> Result<Option<Self>> {
        let episode = episode::Entity::find()
            .filter(episode::Column::Guid.eq(&key.guid))
            .filter(episode::Column::PodcastXmlUrl.eq(&key.podcast_xml_url))
            .one(db())
            .await?;
        Ok(episode)
    }

    async fn get_path_by_key(key: &Self::Key) -> Result<Option<PathBuf>> {
        if let Some(episode) = Self::get_by_key(key).await? {
            Ok(episode.file_path.map(PathBuf::from))
        } else {
            Ok(None)
        }
    }

    async fn download_by_key(
        key: &Self::Key,
        opts: &DownloadOptions,
        uuid: Uuid,
        tx: mpsc::Sender<DownloadMessages>,
    ) -> Result<()> {
        let mut tf = TempFile::new().await?;
        let episode = Self::get_by_key(key).await?
            .ok_or_else(|| anyhow::anyhow!("Episode not found"))?;

        let response = create_client().get(&episode.enclosure_url).send().await?;
        let total_size = response.content_length().unwrap_or(0);
        let _ = tx.send(DownloadMessages::StartDownload {
            uuid,
            total_size: total_size as usize,
            title: episode.title.clone(),
            author: episode.podcast_title.clone(),
        }).await;

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

        let _ = tx.send(DownloadMessages::DownloadProgress { uuid, position: downloaded }).await;
        let _ = tx.send(DownloadMessages::FlushingDownload { uuid }).await;
        let _ = tf.flush().await;
        let _ = tx.send(DownloadMessages::AnalyzingMedia { uuid }).await;

        let (track_gain, track_peak) = replaygain_analysis(tf.file_path()).await?;
        let ffprobe_output = FfprobeOutput::from_file(tf.file_path().to_str().unwrap()).await?;

        let _ = tx.send(DownloadMessages::StartTranscodeAndCompress {
            uuid,
            compressing: opts.compress,
            length_seconds: ffprobe_output.duration_seconds().unwrap_or_default(),
        }).await;

        let output_path = PathBuf::from_str(episode.file_path.as_ref().unwrap())?;
        let _ = create_dir_all(output_path.parent().unwrap()).await;

        let cmd = if opts.compress {
            transcode_command(tf.file_path(), &output_path, crate::ffmpeg::TranscodeOptions::Compress)
        } else if ffprobe_output.format.and_then(|f| f.format_name).is_some_and(|f| f != "mp3") {
            transcode_command(tf.file_path(), &output_path, crate::ffmpeg::TranscodeOptions::Default)
        } else {
            transcode_command(tf.file_path(), &output_path, crate::ffmpeg::TranscodeOptions::Copy)
        };

        ffmpeg_channel_reporter(cmd, uuid, tx.clone()).await?;

        write_replay_gain(&output_path, track_gain, track_peak)?;
        let _ = tx.send(DownloadMessages::Finish { uuid }).await;

        Ok(())
    }
}

async fn ffmpeg_channel_reporter(
    mut cmd: FfmpegCommand,
    uuid: Uuid,
    tx: mpsc::Sender<DownloadMessages>,
) -> Result<()> {
    let mut stream = cmd.spawn()?.stream()?;
    while let Some(event) = stream.next().await {
        match event {
            FfmpegEvent::Log(_, s) => {
                if s.starts_with("out_time=") {
                    let t = Time::from_str(s.split_once("=").unwrap().1).unwrap();
                    let seconds = t.hour as f64 * 3600.0
                        + t.minute as f64 * 60.0
                        + t.second as f64
                        + t.microsecond as f64 / 1_000_000.0;
                    let _ = tx.try_send(DownloadMessages::TranscodeAndCompressProgress {
                        uuid,
                        position: seconds,
                    });
                }
            },
            _ => {}
        }
    }
    Ok(())
}


impl WithSafeFilename for episode::Model {
    fn safe_filename(&self) -> String {
        format!("{}.mp3", sanitize_filename(&self.title))
    }
}

impl WithSafeFilename for podcast::Model {
    fn safe_filename(&self) -> String {
        sanitize_filename(&self.title)
    }
}

impl WithSafeFilename for youtube_playlist::Model {
    fn safe_filename(&self) -> String {
        sanitize_filename(&self.title)
    }
}

impl WithSafeFilename for youtube_video::Model {
    fn safe_filename(&self) -> String {
        format!("{}.mp3", sanitize_filename(&self.title))
    }
}
