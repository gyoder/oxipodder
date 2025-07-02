pub mod types;
pub mod helpers;
pub mod downloader;

use std::{fmt::write, fs};
use std::path::Path;
use anyhow::{Context, Result};
use helpers::sanitize_filename;
use types::PodderDB;

pub const DB_FILE_NAME: &str = "podder_db.json";
pub const PODCAST_DIR: &str = "podcasts";

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

