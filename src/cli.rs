use anyhow::{anyhow, Context, Result};
use clap::Subcommand;
use futures::future::join_all;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use url::Url;
use uuid::Uuid;
use std::collections::HashMap;
use std::time::Duration;
use sea_orm::{
    prelude::*, ActiveModelTrait, EntityTrait, QueryFilter, QuerySelect
};

use crate::db;
use crate::models::settings::{self, get_settings};
use crate::models::{episode, podcast, Collection, Content, ContentDownloader, DownloadMessages, DownloadOptions, DownloadQueueElement, Library, PodcastLibrary};

#[derive(Subcommand)]
pub enum Commands {
    Create {
        #[arg(short, long)]
        opml: Option<String>,
    },
    Add {
        #[arg(index = 1)]
        url: String,
    },
    Update {
        #[arg(short, long)]
        download: bool,
    },
    Download {
        #[arg(short, long)]
        episodes: Option<usize>,
        #[arg(short, long, default_value = "false")]
        no_episode_limit: bool,
        #[arg(short, long, default_value = "false")]
        download_listened: bool
    },
    DownloadOne {
        #[arg(index = 1)]
        podcast_number: usize,
        #[arg(short, long)]
        episodes: Option<usize>,
        #[arg(short, long, default_value = "false")]
        no_episode_limit: bool,
        #[arg(short, long, default_value = "false")]
        download_listened: bool
    },
    ChangeSettings {
        #[command(subcommand)]
        settings: Settings
    },
    ChangePodcastSettings {
        #[arg()]
        podcast_number: usize,
        #[command(subcommand)]
        settings: PodcastSettings
    },
    List,
    YtAdd {
        #[arg(short, long)]
        url: String,
        #[arg(short, long)]
        name: String,
    },
    YtDownload,
}

#[derive(Subcommand, Debug)]
pub enum Settings {
    Compress {
        #[arg(index = 1)]
        value: String,
    },
    DefaultPodcastLimit {
        #[arg(index = 1)]
        value: String,
    },
    DefaultYoutubeLimit {
        #[arg(index = 1)]
        value: String,
    },
    DownloadThreads {
        #[arg(index = 1)]
        value: u8,
    },
}

#[derive(Subcommand, Debug)]
pub enum PodcastSettings {
    EpisodeLimit {
        #[arg(index = 1)]
        value: String
    },
    AutoDeleteOldEpisodes {
        #[arg(index = 1)]
        value: String
    }
}

pub async fn settings_command(settings: Settings) -> Result<()> {

    let settings_id = 1;

    match settings {
        Settings::Compress { value } => {
            settings::Entity::update_many()
                .col_expr(settings::Column::Compress, Expr::value(value.parse::<bool>()?))
                .filter(settings::Column::Id.eq(settings_id))
                .exec(db())
                .await?;
        },
        Settings::DefaultPodcastLimit { value } => {
            settings::Entity::update_many()
                .col_expr(settings::Column::DefaultEpisodeLimitPodcast, Expr::value(value.parse::<i32>().ok()))
                .filter(settings::Column::Id.eq(settings_id))
                .exec(db())
                .await?;
        },
        Settings::DefaultYoutubeLimit { value } => {
            settings::Entity::update_many()
                .col_expr(settings::Column::DefaultEpisodeLimitYoutube, Expr::value(value.parse::<i32>().ok()))
                .filter(settings::Column::Id.eq(settings_id))
                .exec(db())
                .await?;
        },
        Settings::DownloadThreads { value } => {
            settings::Entity::update_many()
                .col_expr(settings::Column::DownloadThreads, Expr::value(value))
                .filter(settings::Column::Id.eq(settings_id))
                .exec(db())
                .await?;
        },
    }

    println!("Setting Updated");
    Ok(())
}

pub async fn podcast_settings_command(podcast_idx: usize, settings: PodcastSettings) -> Result<()> {

    let podcasts = PodcastLibrary::get_all().await;
    let podcast = podcasts.get(podcast_idx).ok_or(anyhow!("Invalid Index"))?;
    let podcast_xml_url = podcast.xml_url.clone();

    match settings {
        PodcastSettings::EpisodeLimit { value } => {
            podcast::Entity::update_many()
                .col_expr(podcast::Column::AutoDownloadLimit, Expr::value(value.parse::<i32>().ok()))
                .filter(podcast::Column::XmlUrl.eq(podcast_xml_url))
                .exec(db())
                .await?;
        },
        PodcastSettings::AutoDeleteOldEpisodes { value } => {
            podcast::Entity::update_many()
                .col_expr(podcast::Column::DeleteOldEpisodes, Expr::value(value.parse::<bool>()?))
                .filter(podcast::Column::XmlUrl.eq(podcast_xml_url))
                .exec(db())
                .await?;
        },
    }

    println!("Setting Updated");
    Ok(())
}


pub async fn create_command(opml: Option<String>) -> Result<()> {
    println!("Creating podcast database", );
    //
    // // let mut db = PodderDB::from_opml(&opml).await?;
    // let mut db = PodderDB::default();
    // let output_path = Path::new(&output);
    //
    // fs::create_dir_all(output_path)?;
    //
    // // println!("Updating RSS feeds...");
    // // db.update_feeds().await?;
    //
    // db.save(output_path)?;
    // println!("Database created with {} podcasts", db.podcasts.len());
    //
    // // if episodes > 0 {
    // //     println!("Downloading {} episodes per podcast...", episodes);
    // //     download_episodes(&mut db, output_path, episodes).await?;
    // //     db.save(output_path)?;
    // // }
    //
    // println!("Setup complete!");
    Ok(())
}

pub async fn add_command(url: String) -> Result<()> {

    let (title, count) = PodcastLibrary::add_from_url(Url::parse(&url)?)
        .await
        .with_context(|| format!("Failed to add RSS feed: {}", url))?;

    println!("Successfully added podcast {title}: {count} episodes");

    println!("RSS feed added successfully!");
    Ok(())
}

pub async fn update_command(download: bool) -> Result<()> {
    println!("Updating RSS feeds...");
    let new = PodcastLibrary::update_all().await;
    println!("Successfully updated podcasts");
    for n in new {
        if !n.new_content.is_empty() {
            println!("New podcasts from {}", n.collection);
            for title in n.new_content {
                println!("\t{title}");
            }
        }
    }


    if download {
        println!("Downloading new episodes...");
        download_command(None, false, false).await?;
    }

    println!("Update complete!");
    Ok(())
}

pub async fn download_command(episodes: Option<usize>, no_episode_limit: bool, download_listened: bool) -> Result<()> {
    println!(
        "Downloading {} episodes for all podcasts",
        match episodes {
            Some(num) => num.to_string(),
            None => "all".to_string(),
        }
    );

    let s = get_settings().await;
    let opts = DownloadOptions {
        no_auto_delete: download_listened || no_episode_limit || episodes.is_some(),
        override_download_limit: match episodes {
            Some(e) => Some(if no_episode_limit {None} else {Some(e)}),
            None => if no_episode_limit {Some(None)} else {None},
        },
        compress: s.compress,
        download_listened,
    };

    let queue = PodcastLibrary::get_all_download_queue(&opts).await;
    download_screen::<episode::Model>(queue, opts).await;

    println!("Download complete!");
    Ok(())
}

pub async fn download_one_command(podcast_idx: usize, episodes: Option<usize>, no_episode_limit: bool, download_listened: bool) -> Result<()> {
    let podcasts = PodcastLibrary::get_all().await;

    if podcast_idx >= podcasts.len() {
        return Err(anyhow::anyhow!(
            "Podcast index {} out of range (0-{})",
            podcast_idx,
            podcasts.len() - 1
        ));
    }

    println!(
        "Downloading {} episodes for podcast: {}",
        match episodes {
            Some(num) => num.to_string(),
            None => "all".to_string(),
        }, podcasts[podcast_idx].title
    );

    let s = get_settings().await;
    let opts = DownloadOptions {
        no_auto_delete: download_listened || no_episode_limit || episodes.is_some(),
        override_download_limit: match episodes {
            Some(e) => Some(if no_episode_limit {None} else {Some(e)}),
            None => if no_episode_limit {Some(None)} else {None},
        },
        compress: s.compress,
        download_listened,
    };

    let queue = podcasts[podcast_idx].get_download_queue(&opts).await;
    download_screen::<episode::Model>(queue, opts).await;

    println!("Download complete!");
    Ok(())
}

pub async fn list_command() -> Result<()> {
    let podcasts = PodcastLibrary::get_all().await;

    println!("\nPodcasts:");
    for (i, podcast) in podcasts.into_iter().enumerate() {
        let unheard_count = podcast
            .get_all_content().await
            .iter()
            .filter(|e| !e.listened_to)
            .count();
        println!(
            "{}: {} ({} unheard episodes, {} episode download limit{})",
            i, podcast.title, unheard_count, podcast.auto_download_limit.as_ref().map(|e| e.to_string()).unwrap_or("no".to_string()), if podcast.delete_old_episodes {", automatically deletes old episodes"} else {""}
        );
    }

    // if !db.youtube_playlists.is_empty() {
    //     println!("\nYouTube Playlists:");
    //     for (i, playlist) in db.youtube_playlists.iter().enumerate() {
    //         println!("{}: {}", i, playlist.title);
    //     }
    // }

    Ok(())
}

pub async fn yt_add_command(url: String, name: String) -> Result<()> {
    todo!();
    // let base_path = Path::new(&path);
    // let mut db = load_db(base_path, &path)?;
    //
    // db.youtube_playlists.push(YoutubePlaylist {
    //     title: name.clone(),
    //     url: url.clone(),
    // });
    //
    // db.save(base_path)?;
    println!("Added YouTube playlist: {} ({})", name, url);
    Ok(())
}

pub async fn yt_download_command() -> Result<()> {
    todo!();
    // let base_path = Path::new(&path);
    // let db = load_db(base_path, &path)?;
    //
    // if db.youtube_playlists.is_empty() {
    //     println!("No YouTube playlists configured");
    //     return Ok(());
    // }
    //
    // let youtube_dir = base_path.join("youtube_playlists");
    // fs::create_dir_all(&youtube_dir)?;

    // for playlist in &db.youtube_playlists {
    //     let playlist_dir = youtube_dir.join(playlist.safe_filename());
    //     fs::create_dir_all(&playlist_dir)?;
    //
    //     println!("Downloading playlist: {}", playlist.title);
    //
    //     let status = tokio::process::Command::new("yt-dlp")
    //         .args([
    //             "-x",
    //             "--audio-format",
    //             "mp3",
    //             "--embed-thumbnail",
    //             "--add-metadata",
    //         ])
    //         .args(["-o", "%(playlist_index)s - %(title)s.%(ext)s"])
    //         .arg(&playlist.url)
    //         .args(["--download-archive", "downloaded.txt"])
    //         .current_dir(&playlist_dir)
    //         .status()
    //         .await?;
    //
    //     if status.success() {
    //         println!("Successfully downloaded playlist: {}", playlist.title);
    //     } else {
    //         eprintln!(
    //             "yt-dlp failed for playlist '{}': {}",
    //             playlist.title, status
    //         );
    //     }
    // }

    println!("YouTube download complete!");
    Ok(())
}

async fn download_screen<CD: ContentDownloader>(
    mut tasks: Vec<DownloadQueueElement<CD::Key>>,
    opts: DownloadOptions,
) {
    let (task_queue_tx, task_queue_rx) = flume::unbounded::<DownloadQueueElement<CD::Key>>();
    let s = get_settings().await;

    // Send all tasks to the queue
    while let Some(e) = tasks.pop() {
        task_queue_tx.send(e).unwrap();
    }
    drop(task_queue_tx);

    let (msg_tx, mut msg_rx) = mpsc::channel::<DownloadMessages>(10000);
    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    let multi_progress = MultiProgress::new();
    let progress_bars = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::<Uuid, (ProgressBar, String, String)>::new()));

    for _i in 0..s.download_threads {
        let task_queue_rx = task_queue_rx.clone();
        let msg_tx = msg_tx.clone();
        let opts = opts.clone();

        handles.push(tokio::task::spawn(async move {
            while let Ok(task) = task_queue_rx.recv_async().await {
                if let Err(e) = CD::Content::download_by_key(
                    &task.content_key,
                    &opts,
                    task.uuid,
                    msg_tx.clone()
                ).await {
                    eprintln!("Download failed for task {}: {}", task.uuid, e);
                    // todo: wipe download path from db
                }
            }
        }));
    }

    let progress_bars_clone = progress_bars.clone();
    let multi_progress_clone = multi_progress.clone();

    let message_handler = tokio::task::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            let mut bars = progress_bars_clone.lock().await;

            match msg {
                DownloadMessages::StartDownload { uuid, total_size, title, author } => {
                    let pb = multi_progress_clone.add(ProgressBar::new(total_size as u64));
                    pb.set_style(
                        ProgressStyle::default_bar()
                            .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}")
                            .unwrap()
                            .progress_chars("#>-")
                    );
                    pb.set_message(format!("{} - {}", title, author));
                    pb.enable_steady_tick(Duration::from_millis(120));
                    bars.insert(uuid, (pb, title, author));
                }
                DownloadMessages::DownloadProgress { uuid, position } => {
                    if let Some((pb, _, _)) = bars.get(&uuid) {
                        pb.set_position(position as u64);
                    }
                }
                DownloadMessages::FlushingDownload { uuid } => {
                    if let Some((pb, title, author)) = bars.get(&uuid) {
                        pb.set_style(
                            ProgressStyle::default_bar()
                            .template("[{bar:42.ret/red}] {msg}")
                            .unwrap()
                            .progress_chars("xxx")
                        );
                        pb.set_position(0);
                        pb.set_message(format!("Flushing: {title} - {author}"));
                    }
                }
                DownloadMessages::AnalyzingMedia { uuid } => {
                    if let Some((pb, title, author)) = bars.get(&uuid) {
                        pb.set_style(
                            ProgressStyle::default_bar()
                            .template("[{bar:42.green/green}] {msg}")
                            .unwrap()
                            .progress_chars("xxx")
                        );
                        pb.set_position(0);
                        pb.set_message(format!("Analyzing: {title} - {author}"));
                    }
                }
                DownloadMessages::StartTranscodeAndCompress { uuid, compressing, length_seconds } => {
                    if let Some((pb, title, author)) = bars.get(&uuid) {
                        let total_seconds = length_seconds as u64;
                        pb.set_length(total_seconds);
                        pb.set_position(0);

                        let style = if compressing {
                            ProgressStyle::default_bar()
                                .template("{spinner:.green} [{bar:40.yellow/red}] {msg}")
                                .unwrap()
                                .progress_chars("#>-")
                        } else {
                            ProgressStyle::default_bar()
                                .template("{spinner:.green} [{bar:40.blue/cyan}] {msg}")
                                .unwrap()
                                .progress_chars("#>-")
                        };

                        pb.set_style(style);
                        pb.set_message(format!("0m0s/{} {}: {} - {}", format_duration(total_seconds), if compressing {"Compressing"} else {"Transcoding"}, title, author));
                    }
                }
                DownloadMessages::TranscodeAndCompressProgress { uuid, position } => {
                    if let Some((pb, _, _)) = bars.get(&uuid) {
                        pb.set_position(position as u64);
                        pb.set_message(replace_before_char_split(&pb.message(), '/', &format_duration(position as u64)));
                    }
                }
                DownloadMessages::Finish { uuid } => {
                    if let Some((pb, title, author)) = bars.get(&uuid) {
                        pb.finish_with_message(format!("Completed: {title} - {author}"));
                    }
                    bars.remove(&uuid);
                }
            }

        }
    });

    join_all(handles).await;

    drop(msg_tx);
    let _ = message_handler.await;

    let bars = progress_bars.lock().await;
    for (_, (pb, _, _)) in bars.iter() {
        pb.finish_and_clear();
    }
    drop(bars);

    println!("All downloads completed!");
}


fn format_duration(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{}h{:02}m{:02}s", hours, minutes, seconds)
    } else {
        format!("{}m{:02}s", minutes, seconds)
    }
}

fn replace_before_char_split(text: &str, separator: char, replacement: &str) -> String {
    match text.split_once(separator) {
        Some((_, after)) => format!("{}{}{}", replacement, separator, after),
        None => text.to_string(),
    }
}
