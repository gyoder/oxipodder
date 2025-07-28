pub mod types;
pub mod helpers;
pub mod downloader;

use std::process::Command;
use std::{fmt::write, fs};
use std::path::Path;
use anyhow::{Context, Result};
use helpers::sanitize_filename;
use types::PodderDB;

pub const DB_FILE_NAME: &str = "podder_db.json";
pub const PODCAST_DIR: &str = "podcasts";

pub fn read_podder_db(base_path: &str) -> Result<PodderDB> {
    let base_path = Path::new(base_path);
    let db_file_path = base_path.join(DB_FILE_NAME);

    if !db_file_path.exists() {
        return Err(anyhow::anyhow!("podder_db.json not found at {:?}", db_file_path));
    }

    let db_content = fs::read_to_string(&db_file_path)
        .context("Failed to read podder_db.json")?;

    let mut podder_db: PodderDB = serde_json::from_str(&db_content)
        .context("Failed to parse podder_db.json")?;

    Ok(podder_db)
}


pub fn save_podder_db(base_path: &str, podder_db: PodderDB) -> Result<()> {
    let base_path = Path::new(base_path);
    let final_db_content = serde_json::to_string_pretty(&podder_db)
        .context("Failed to serialize final database")?;
    fs::write(base_path.join(DB_FILE_NAME), final_db_content)
        .context("Failed to save final database")?;

    Ok(())
}

pub fn process_podcasts(base_path: &str) -> Result<PodderDB> {
    let base_path = Path::new(base_path);
    let db_file_path = base_path.join(DB_FILE_NAME);

    if !db_file_path.exists() {
        return Err(anyhow::anyhow!("podder_db.json not found at {:?}", db_file_path));
    }

    let db_content = fs::read_to_string(&db_file_path)
        .context("Failed to read podder_db.json")?;

    let mut podder_db: PodderDB = serde_json::from_str(&db_content)
        .context("Failed to parse podder_db.json")?;

    let podcasts_dir = base_path.join(PODCAST_DIR);
    if !podcasts_dir.exists() {
        fs::create_dir_all(&podcasts_dir)
            .context("Failed to create podcasts directory")?;
        println!("Created podcasts directory at {:?}", podcasts_dir);
    }

    for podcast in &podder_db.podcasts {
        let dir_name = podcast.filename();
        let podcast_dir = podcasts_dir.join(&dir_name);

        if !podcast_dir.exists() {
            fs::create_dir_all(&podcast_dir)
                .with_context(|| format!("Failed to create directory for podcast: {}", podcast.title))?;
            println!("Created directory for podcast: {} at {:?}", podcast.title, podcast_dir);
        }
    }

    podder_db.update_rss_feeds()
        .context("Failed to update RSS feeds")?;


    for pod in &mut podder_db.podcasts {
        let pod_dir = podcasts_dir.join(pod.filename());
        for episode in &mut pod.episodes {
            if episode.downloaded_on_last_sync {
                let episode_file = pod_dir.join(episode.filename());
                if !episode_file.exists() {
                    episode.downloaded_on_last_sync = false;
                    episode.listened_to = true;
                }
            }
        }
    }

    println!("Successfully processed {} podcasts and updated RSS feeds", podder_db.podcasts.len());

    Ok(podder_db)
}


pub fn download_youtube_playlists(podder_db: &mut PodderDB, base_path: &str) -> Result<()> {
    let base_path = Path::new(base_path);

    for playlist in &mut podder_db.youtube_playlists {
        let playlist_dir = base_path.join("youtube_playlists").join(playlist.filename());
        std::fs::create_dir_all(&playlist_dir)?;

        let output_template = "%(playlist_index)s - %(title)s.%(ext)s";
        let archive_file = "a_downloaded.txt";

        let status = Command::new("yt-dlp")
            .arg("-x")
            .arg("--audio-format")
            .arg("mp3")
            .arg("--embed-thumbnail")
            .arg("--add-metadata")
            .arg("-o")
            .arg(output_template)
            .arg(&playlist.url)
            .arg("--download-archive")
            .arg(archive_file)
            .current_dir(&playlist_dir)
            .status()?;

        if !status.success() {
            eprintln!(
                "yt-dlp failed for playlist '{}' ({}): status {}",
                playlist.title,
                playlist.url,
                status.code().unwrap_or(-1)
            );
        }
    }

    Ok(())
}



