use anyhow::{Context, Result};
use clap::Subcommand;
use futures::future::join_all;
use tokio::task::JoinHandle;
use url::Url;
use std::{fs, path::Path};

use crate::models::{Content, DownloadMessages, DownloadQueueElement, Library, PodderDB};
use crate::models::YoutubePlaylist;

#[derive(Subcommand)]
pub enum Commands {
    Create {
        #[arg(short, long)]
        opml: Option<String>,
        #[arg(short = 'O', long, default_value = ".")]
        output: String,
        #[arg(short, long, default_value = "5")]
        episodes: usize,
    },
    Add {
        #[arg(short, long, default_value = ".")]
        path: String,
        #[arg(short, long)]
        url: String,
    },
    Update {
        #[arg(short, long, default_value = ".")]
        path: String,
        #[arg(short, long)]
        download: bool,
    },
    Download {
        #[arg(short, long, default_value = ".")]
        path: String,
        #[arg(short, long, default_value = "5")]
        episodes: usize,
    },
    DownloadOne {
        #[arg(short, long, default_value = ".")]
        path: String,
        #[arg(short, long)]
        podcast: usize,
        #[arg(short, long, default_value = "1000")]
        episodes: usize,
    },
    List {
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    YtAdd {
        #[arg(short, long, default_value = ".")]
        path: String,
        #[arg(short, long)]
        url: String,
        #[arg(short, long)]
        name: String,
    },
    YtDownload {
        #[arg(short, long, default_value = ".")]
        path: String,
    },
}

pub async fn create_command(opml: Option<String>, output: String, episodes: usize) -> Result<()> {
    println!("Creating podcast database", );

    // let mut db = PodderDB::from_opml(&opml).await?;
    let mut db = PodderDB::default();
    let output_path = Path::new(&output);

    fs::create_dir_all(output_path)?;

    // println!("Updating RSS feeds...");
    // db.update_feeds().await?;

    db.save(output_path)?;
    println!("Database created with {} podcasts", db.podcasts.len());

    // if episodes > 0 {
    //     println!("Downloading {} episodes per podcast...", episodes);
    //     download_episodes(&mut db, output_path, episodes).await?;
    //     db.save(output_path)?;
    // }

    println!("Setup complete!");
    Ok(())
}

pub async fn add_command(path: String, url: String) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = load_db(base_path, &path)?;

    let (title, count) = db.podcasts.add_from_url(Url::parse(&url)?)
        .await
        .with_context(|| format!("Failed to add RSS feed: {}", url))?;

    println!("Successfully added podcast {title}: {count} episodes");

    db.save(base_path)?;
    println!("RSS feed added successfully!");
    Ok(())
}

pub async fn update_command(path: String, download: bool) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = load_db(base_path, &path)?;

    println!("Updating RSS feeds...");
    let new = db.podcasts.update_all(&base_path.to_path_buf()).await;
    println!("Successfuly updated podcasts");
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
        download_episodes(&mut db, base_path, 5).await?;
    }

    db.save(base_path)?;
    println!("Update complete!");
    Ok(())
}

pub async fn download_command(path: String, episodes: usize) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = load_db(base_path, &path)?;

    println!("Downloading {} episodes per podcast...", episodes);
    download_episodes(&mut db, base_path, episodes).await?;

    db.save(base_path)?;
    println!("Download complete!");
    Ok(())
}

pub async fn download_one_command(path: String, podcast_idx: usize, episodes: usize) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = load_db(base_path, &path)?;

    if podcast_idx >= db.podcasts.len() {
        return Err(anyhow::anyhow!(
            "Podcast index {} out of range (0-{})",
            podcast_idx,
            db.podcasts.len() - 1
        ));
    }

    println!(
        "Downloading {} episodes for podcast: {}",
        episodes, db.podcasts[podcast_idx].title
    );
    download_episodes_filtered(&mut db, base_path, episodes, Some(podcast_idx)).await?;

    db.save(base_path)?;
    println!("Download complete!");
    Ok(())
}

pub async fn list_command(path: String) -> Result<()> {
    let base_path = Path::new(&path);
    let db = load_db(base_path, &path)?;

    println!("\nPodcasts:");
    for (i, podcast) in db.podcasts.iter().enumerate() {
        let unheard_count = podcast
            .episodes
            .iter()
            .filter(|e| !e.listened_to)
            .count();
        println!(
            "{}: {} ({} unheard episodes)",
            i, podcast.title, unheard_count
        );
    }

    if !db.youtube_playlists.is_empty() {
        println!("\nYouTube Playlists:");
        for (i, playlist) in db.youtube_playlists.iter().enumerate() {
            println!("{}: {}", i, playlist.title);
        }
    }

    db.save(base_path)?;
    Ok(())
}

pub async fn yt_add_command(path: String, url: String, name: String) -> Result<()> {
    todo!();
    let base_path = Path::new(&path);
    let mut db = load_db(base_path, &path)?;

    // db.youtube_playlists.push(YoutubePlaylist {
    //     title: name.clone(),
    //     url: url.clone(),
    // });

    db.save(base_path)?;
    println!("Added YouTube playlist: {} ({})", name, url);
    Ok(())
}

pub async fn yt_download_command(path: String) -> Result<()> {
    todo!();
    let base_path = Path::new(&path);
    let db = load_db(base_path, &path)?;

    if db.youtube_playlists.is_empty() {
        println!("No YouTube playlists configured");
        return Ok(());
    }

    let youtube_dir = base_path.join("youtube_playlists");
    fs::create_dir_all(&youtube_dir)?;

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

fn load_db(base_path: &Path, path: &str) -> Result<PodderDB> {
    PodderDB::load(base_path)
        .with_context(|| format!("Failed to load database from path: {}", path))
}


async fn download_screen<T: Content>(mut tasks: Vec<DownloadQueueElement<T>>) {
    let (task_queue_tx, task_queue_rx) = flume::unbounded::<DownloadQueueElement<T>>();
    while let Some(e) = tasks.pop() {
        task_queue_tx.send(e).unwrap();
    }
    drop(task_queue_tx);

    let (msg_tx, msg_rx) = flume::unbounded::<DownloadMessages>();

    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    for i in 0..1 {
        let task_queue_rx = task_queue_rx.clone();
        let msg_tx = msg_tx.clone();
        handles.push(tokio::task::spawn(async move {
            while let Ok(task) = task_queue_rx.recv_async().await {
                // handle task
            }
        }));
    }

    join_all(handles).await;

}
