use anyhow::{Context, Result};
use clap::{Arg, Command};
use opml::OPML;
use oxipodder_backend::process_podcasts;
use oxipodder_backend::types::PodderDB;
use reqwest::blocking::Client;
use std::fs;
use std::path::{Path, PathBuf};
use std::io::Write;
use url::Url;

fn main() -> Result<()> {
    let matches = Command::new("oxipodder")
        .version("0.1.0")
        .author("Your Name")
        .about("A podcast downloader and manager")
        .subcommand(
            Command::new("create")
                .about("Create a new podcast database from OPML file")
                .arg(
                    Arg::new("opml")
                        .long("opml")
                        .short('o')
                        .value_name("FILE")
                        .help("Path to OPML file containing podcast subscriptions")
                        .required(true),
                )
                .arg(
                    Arg::new("output")
                        .long("output")
                        .short('O')
                        .value_name("DIR")
                        .help("Output directory for the podcast database")
                        .default_value("."),
                )
                .arg(
                    Arg::new("episodes")
                        .long("episodes")
                        .short('e')
                        .value_name("NUMBER")
                        .help("Number of latest episodes to download per podcast")
                        .default_value("5"),
                )
                .arg(
                    Arg::new("auto-download-limit")
                        .long("auto-download-limit")
                        .short('a')
                        .value_name("NUMBER")
                        .help("Set auto download limit for each podcast")
                        .default_value("5"),
                ),
        )
        .subcommand(
            Command::new("update")
                .about("Update existing podcast database")
                .arg(
                    Arg::new("path")
                        .long("path")
                        .short('p')
                        .value_name("DIR")
                        .help("Path to podcast database directory")
                        .default_value("."),
                )
                .arg(
                    Arg::new("download")
                        .long("download")
                        .short('d')
                        .help("Download new episodes after updating feeds")
                        .action(clap::ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("download")
                .about("Download episodes from existing database")
                .arg(
                    Arg::new("path")
                        .long("path")
                        .short('p')
                        .value_name("DIR")
                        .help("Path to podcast database directory")
                        .default_value("."),
                )
                .arg(
                    Arg::new("episodes")
                        .long("episodes")
                        .short('e')
                        .value_name("NUMBER")
                        .help("Number of episodes to download per podcast")
                        .default_value("5"),
                ),
        )
        .get_matches();

    match matches.subcommand() {
        Some(("create", sub_matches)) => {
            let opml_path = sub_matches.get_one::<String>("opml").unwrap();
            let output_dir = sub_matches.get_one::<String>("output").unwrap();
            let episodes_count: usize = sub_matches
                .get_one::<String>("episodes")
                .unwrap()
                .parse()
                .context("Invalid episodes number")?;
            let auto_download_limit: i32 = sub_matches
                .get_one::<String>("auto-download-limit")
                .unwrap()
                .parse()
                .context("Invalid auto download limit")?;

            create_podderdb_from_opml(opml_path, output_dir, episodes_count, auto_download_limit)?;
        }
        Some(("update", sub_matches)) => {
            let path = sub_matches.get_one::<String>("path").unwrap();
            let should_download = sub_matches.get_flag("download");

            update_podderdb(path, should_download)?;
        }
        Some(("download", sub_matches)) => {
            let path = sub_matches.get_one::<String>("path").unwrap();
            let episodes_count: usize = sub_matches
                .get_one::<String>("episodes")
                .unwrap()
                .parse()
                .context("Invalid episodes number")?;

            download_episodes(path, episodes_count)?;
        }
        _ => {
            println!("No subcommand provided. Use --help for usage information.");
        }
    }

    Ok(())
}

fn create_podderdb_from_opml(
    opml_path: &str,
    output_dir: &str,
    episodes_count: usize,
    auto_download_limit: i32,
) -> Result<()> {
    println!("Creating podcast database from OPML file: {}", opml_path);

    let opml_content = fs::read_to_string(opml_path)
        .with_context(|| format!("Failed to read OPML file: {}", opml_path))?;

    let opml = OPML::from_str(&opml_content)
        .context("Failed to parse OPML file")?;

    let mut podder_db = PodderDB::create_from_opml(opml)
        .context("Failed to create PodderDB from OPML")?;

    for podcast in &mut podder_db.podcasts {
        podcast.auto_download_limit = Some(auto_download_limit);
    }

    println!("Found {} podcasts in OPML file", podder_db.podcasts.len());

    let output_path = Path::new(output_dir);
    fs::create_dir_all(output_path)
        .with_context(|| format!("Failed to create output directory: {}", output_dir))?;

    println!("Updating RSS feeds...");
    podder_db.update_rss_feeds()
        .context("Failed to update RSS feeds")?;

    let podcasts_dir = output_path.join("podcasts");
    fs::create_dir_all(&podcasts_dir)
        .context("Failed to create podcasts directory")?;

    // Create directories for each podcast
    for podcast in &podder_db.podcasts {
        let dir_name = sanitize_filename(&podcast.title);
        let podcast_dir = podcasts_dir.join(&dir_name);

        fs::create_dir_all(&podcast_dir)
            .with_context(|| format!("Failed to create directory for podcast: {}", podcast.title))?;

        println!("Created directory for podcast: {}", podcast.title);
    }

    // Save the database
    let db_file_path = output_path.join("podder_db.json");
    let db_content = serde_json::to_string_pretty(&podder_db)
        .context("Failed to serialize database")?;

    fs::write(&db_file_path, db_content)
        .with_context(|| format!("Failed to write database file: {:?}", db_file_path))?;

    println!("Database created successfully at: {:?}", db_file_path);

    // Download episodes
    if episodes_count > 0 {
        println!("Downloading {} episodes per podcast...", episodes_count);
        download_episodes_from_db(&mut podder_db, &podcasts_dir, episodes_count)?;

        // Save updated database with download status
        let updated_db_content = serde_json::to_string_pretty(&podder_db)
            .context("Failed to serialize updated database")?;

        fs::write(&db_file_path, updated_db_content)
            .context("Failed to save updated database")?;
    }

    println!("Podcast database created successfully!");
    Ok(())
}

fn update_podderdb(path: &str, should_download: bool) -> Result<()> {
    println!("Updating podcast database at: {}", path);

    let base_path = Path::new(path);
    let mut podder_db = process_podcasts(path)?;

    println!("RSS feeds updated successfully!");

    if should_download {
        let podcasts_dir = base_path.join("podcasts");
        let episodes_count = 5; // Default download count

        println!("Downloading new episodes...");
        download_episodes_from_db(&mut podder_db, &podcasts_dir, episodes_count)?;

        // Save database again with download status
        let final_db_content = serde_json::to_string_pretty(&podder_db)
            .context("Failed to serialize final database")?;

        fs::write(base_path.join("podder_db.json"), final_db_content)
            .context("Failed to save final database")?;
    }

    Ok(())
}

fn download_episodes(path: &str, episodes_count: usize) -> Result<()> {
    println!("Downloading episodes from database at: {}", path);

    let base_path = Path::new(path);
    let db_file_path = base_path.join("podder_db.json");

    if !db_file_path.exists() {
        return Err(anyhow::anyhow!("podder_db.json not found at {:?}", db_file_path));
    }

    let db_content = fs::read_to_string(&db_file_path)
        .context("Failed to read podder_db.json")?;

    let mut podder_db: PodderDB = serde_json::from_str(&db_content)
        .context("Failed to parse podder_db.json")?;

    let podcasts_dir = base_path.join("podcasts");

    download_episodes_from_db(&mut podder_db, &podcasts_dir, episodes_count)?;

    let updated_db_content = serde_json::to_string_pretty(&podder_db)
        .context("Failed to serialize updated database")?;

    fs::write(&db_file_path, updated_db_content)
        .context("Failed to save updated database")?;

    Ok(())
}

fn download_episodes_from_db(
    podder_db: &mut PodderDB,
    podcasts_dir: &Path,
    episodes_count: usize,
) -> Result<()> {
    // TODO: do async downloading
    let client = Client::new();

    for podcast in &mut podder_db.podcasts {
        let dir_name = sanitize_filename(&podcast.title);
        let podcast_dir = podcasts_dir.join(&dir_name);

        fs::create_dir_all(&podcast_dir)
            .with_context(|| format!("Failed to create directory for podcast: {}", podcast.title))?;

        podcast.episodes.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));

        let episodes_to_download = podcast.episodes
            .iter_mut()
            .filter(|e| !e.downloaded_on_last_sync && !e.listened_to)
            .take(episodes_count);

        for episode in episodes_to_download {
            let episode_filename = format!("{}.mp3", sanitize_filename(&episode.title));
            let episode_path = podcast_dir.join(&episode_filename);

            if episode_path.exists() {
                episode.downloaded_on_last_sync = true;
                continue;
            }

            println!("Downloading: {} - {}", podcast.title, episode.title);

            match download_episode(&client, &episode.enclosure.url, &episode_path) {
                Ok(_) => {
                    episode.downloaded_on_last_sync = true;
                    println!("✓ Downloaded: {}", episode.title);
                }
                Err(e) => {
                    eprintln!("✗ Failed to download {}: {}", episode.title, e);
                }
            }
        }

        println!("Completed downloads for: {}", podcast.title);
    }

    Ok(())
}

fn download_episode(client: &Client, url: &str, output_path: &Path) -> Result<()> {
    let response = client.get(url)
        .send()
        .context("Failed to send download request")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Download failed with status: {}", response.status()));
    }

    let mut file = fs::File::create(output_path)
        .with_context(|| format!("Failed to create file: {:?}", output_path))?;

    let content = response.bytes()
        .context("Failed to read response content")?;

    file.write_all(&content)
        .context("Failed to write episode content to file")?;

    Ok(())
}

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
