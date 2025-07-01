pub mod types;

use std::{fmt::write, fs};
use std::path::Path;
use anyhow::{Context, Result};
use types::PodderDB;

pub fn process_podcasts(base_path: &str) -> Result<PodderDB> {
    let base_path = Path::new(base_path);
    let db_file_path = base_path.join("podder_db.json");

    if !db_file_path.exists() {
        return Err(anyhow::anyhow!("podder_db.json not found at {:?}", db_file_path));
    }

    let db_content = fs::read_to_string(&db_file_path)
        .context("Failed to read podder_db.json")?;

    let mut podder_db: PodderDB = serde_json::from_str(&db_content)
        .context("Failed to parse podder_db.json")?;

    let podcasts_dir = base_path.join("podcasts");
    if !podcasts_dir.exists() {
        fs::create_dir_all(&podcasts_dir)
            .context("Failed to create podcasts directory")?;
        println!("Created podcasts directory at {:?}", podcasts_dir);
    }

    for podcast in &podder_db.podcasts {
        let dir_name = sanitize_filename(&podcast.title);
        let podcast_dir = podcasts_dir.join(&dir_name);

        if !podcast_dir.exists() {
            fs::create_dir_all(&podcast_dir)
                .with_context(|| format!("Failed to create directory for podcast: {}", podcast.title))?;
            println!("Created directory for podcast: {} at {:?}", podcast.title, podcast_dir);
        }
    }

    podder_db.update_rss_feeds()
        .context("Failed to update RSS feeds")?;

    // Save the updated database back to file
    let updated_db_content = serde_json::to_string_pretty(&podder_db)
        .context("Failed to serialize updated database")?;


    println!("Successfully processed {} podcasts and updated RSS feeds", podder_db.podcasts.len());

    Ok(podder_db)
}

// Helper function to sanitize filenames for directory creation
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

