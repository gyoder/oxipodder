use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    sync::Arc,
};
use tokio::io::AsyncWriteExt;
use url::Url;

const DB_FILE_NAME: &str = "podder_db.json";
const PODCAST_DIR: &str = "podcasts";

#[derive(Parser)]
#[command(name = "oxipodder")]
#[command(about = "A fast and simple podcast downloader")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new podcast database from OPML file
    Create {
        /// Path to OPML file
        #[arg(short, long)]
        opml: String,
        /// Output directory
        #[arg(short = 'O', long, default_value = ".")]
        output: String,
        /// Number of episodes to download per podcast
        #[arg(short, long, default_value = "5")]
        episodes: usize,
    },
    /// Update existing podcast database
    Update {
        /// Path to podcast database directory
        #[arg(short, long, default_value = ".")]
        path: String,
        /// Download new episodes after updating
        #[arg(short, long)]
        download: bool,
    },
    /// Download episodes from existing database
    Download {
        /// Path to podcast database directory
        #[arg(short, long, default_value = ".")]
        path: String,
        /// Number of episodes to download per podcast
        #[arg(short, long, default_value = "5")]
        episodes: usize,
    },
    /// Download specific podcast by index
    DownloadOne {
        /// Path to podcast database directory
        #[arg(short, long, default_value = ".")]
        path: String,
        /// Index of podcast to download
        #[arg(short, long)]
        podcast: usize,
        /// Number of episodes to download
        #[arg(short, long, default_value = "1000")]
        episodes: usize,
    },
    /// List all podcasts
    List {
        /// Path to podcast database directory
        #[arg(short, long, default_value = ".")]
        path: String,
    },
    /// Add YouTube playlist
    YtAdd {
        /// Path to podcast database directory
        #[arg(short, long, default_value = ".")]
        path: String,
        /// Playlist URL
        #[arg(short, long)]
        url: String,
        /// Playlist name
        #[arg(short, long)]
        name: String,
    },
    /// Download YouTube playlists
    YtDownload {
        /// Path to podcast database directory
        #[arg(short, long, default_value = ".")]
        path: String,
    },
}

#[derive(Serialize, Deserialize, Default)]
struct PodderDB {
    #[serde(default)]
    podcasts: Vec<Podcast>,
    #[serde(default)]
    youtube_playlists: Vec<YoutubePlaylist>,
}

#[derive(Serialize, Deserialize)]
struct Podcast {
    title: String,
    description: Option<String>,
    xml_url: Url,
    html_url: Option<Url>,
    auto_download_limit: Option<i32>,
    episodes: Vec<Episode>,
    last_refreshed: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct Episode {
    guid: String,
    title: String,
    enclosure: Enclosure,
    pub_date: DateTime<Utc>,
    #[serde(alias = "downloaded")]
    downloaded_on_last_sync: bool,
    listened_to: bool,
}

#[derive(Serialize, Deserialize)]
struct Enclosure {
    url: String,
    length: i32,
    mime_type: String,
}

#[derive(Serialize, Deserialize)]
struct YoutubePlaylist {
    title: String,
    url: String,
}

impl Podcast {
    fn safe_filename(&self) -> String {
        sanitize_filename(&self.title)
    }
}

impl Episode {
    fn safe_filename(&self) -> String {
        format!("{}.mp3", sanitize_filename(&self.title))
    }
}

impl YoutubePlaylist {
    fn safe_filename(&self) -> String {
        sanitize_filename(&self.title)
    }
}

impl PodderDB {
    async fn from_opml(opml_path: &str) -> Result<Self> {
        let content = fs::read_to_string(opml_path)
            .with_context(|| format!("Failed to read OPML file: {}", opml_path))?;

        let opml = opml::OPML::from_str(&content)
            .context("Failed to parse OPML file")?;

        let mut db = PodderDB::default();

        if let Some(body_outline) = opml.body.outlines.first() {
            for outline in &body_outline.outlines {
                if let (Some(title), Some(xml_url)) = (&outline.title, &outline.xml_url) {
                    match Url::parse(xml_url) {
                        Ok(url) => {
                            let podcast = Podcast {
                                title: title.clone(),
                                description: outline.description.clone(),
                                xml_url: url,
                                html_url: outline.html_url.as_ref().and_then(|u| Url::parse(u).ok()),
                                auto_download_limit: Some(5),
                                episodes: Vec::new(),
                                last_refreshed: Utc::now(),
                            };
                            db.podcasts.push(podcast);
                            println!("Added podcast: {}", title);
                        }
                        Err(e) => eprintln!("Invalid URL for {}: {}", title, e),
                    }
                }
            }
        }

        Ok(db)
    }

    async fn update_feeds(&mut self) -> Result<()> {
        let client = reqwest::Client::new();

        for podcast in &mut self.podcasts {
            println!("Updating feed for: {}", podcast.title);

            match client.get(podcast.xml_url.clone()).send().await {
                Ok(response) => {
                    match response.bytes().await {
                        Ok(content) => {
                            match rss::Channel::read_from(&content[..]) {
                                Ok(channel) => {
                                    for item in channel.items {
                                        let guid = item.guid
                                            .map(|g| g.value)
                                            .unwrap_or_else(|| item.title.clone().unwrap_or_default());

                                        if !podcast.episodes.iter().any(|e| e.guid == guid) {
                                            if let Some(enclosure) = item.enclosure {
                                                let episode = Episode {
                                                    guid,
                                                    title: item.title.unwrap_or_default(),
                                                    enclosure: Enclosure {
                                                        url: enclosure.url,
                                                        length: enclosure.length.parse().unwrap_or(0),
                                                        mime_type: enclosure.mime_type,
                                                    },
                                                    pub_date: item.pub_date
                                                        .and_then(|d| DateTime::parse_from_rfc2822(&d).ok())
                                                        .map(|d| d.into())
                                                        .unwrap_or_else(Utc::now),
                                                    downloaded_on_last_sync: false,
                                                    listened_to: false,
                                                };
                                                podcast.episodes.push(episode);
                                            }
                                        }
                                    }

                                    // Sort episodes by publication date (newest first)
                                    podcast.episodes.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));
                                    podcast.last_refreshed = Utc::now();
                                }
                                Err(e) => eprintln!("Failed to parse RSS for {}: {}", podcast.title, e),
                            }
                        }
                        Err(e) => eprintln!("Failed to download RSS for {}: {}", podcast.title, e),
                    }
                }
                Err(e) => eprintln!("Network error for {}: {}", podcast.title, e),
            }
        }

        Ok(())
    }

    fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialize database")?;

        fs::write(path.join(DB_FILE_NAME), content)
            .context("Failed to write database file")?;

        Ok(())
    }

    fn load(path: &Path) -> Result<Self> {
        let db_path = path.join(DB_FILE_NAME);
        let content = fs::read_to_string(&db_path)
            .with_context(|| format!("Failed to read database at {:?}", db_path))?;

        serde_json::from_str(&content)
            .context("Failed to parse database file")
    }
}

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    progress_bar: &ProgressBar,
) -> Result<()> {
    let response = client.get(url).send().await?;
    let total_size = response.content_length().unwrap_or(0);

    progress_bar.set_length(total_size);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}")
            .unwrap()
            .progress_chars("#>-")
    );

    let mut file = tokio::fs::File::create(path).await?;
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        progress_bar.set_position(downloaded);
    }

    file.flush().await?;
    Ok(())
}

async fn download_episodes(db: &mut PodderDB, base_path: &Path, episode_count: usize) -> Result<()> {
    download_episodes_filtered(db, base_path, episode_count, None).await
}

async fn download_episodes_filtered(
    db: &mut PodderDB,
    base_path: &Path,
    episode_count: usize,
    podcast_filter: Option<usize>
) -> Result<()> {
    let podcasts_dir = base_path.join(PODCAST_DIR);
    fs::create_dir_all(&podcasts_dir)?;

    let client = reqwest::Client::new();
    let multi_progress = Arc::new(MultiProgress::new());
    let mut tasks = Vec::new();

    for (i, podcast) in db.podcasts.iter_mut().enumerate() {
        // Skip if filtering and not the target podcast
        if let Some(filter_idx) = podcast_filter {
            if i != filter_idx {
                continue;
            }
        }

        let podcast_dir = podcasts_dir.join(podcast.safe_filename());
        fs::create_dir_all(&podcast_dir)?;

        // Sort episodes by publication date (newest first) to get the most recent ones
        podcast.episodes.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));

        // Take only the most recent N episodes and filter for undownloaded/unlistened
        let recent_episodes: Vec<_> = podcast.episodes
            .iter_mut()
            .take(episode_count)
            .collect();

        // Check if all recent episodes are already downloaded or listened to
        let all_recent_handled = recent_episodes.iter()
            .all(|e| e.downloaded_on_last_sync || e.listened_to);

        if all_recent_handled {
            println!("Skipping '{}' - all {} most recent episodes already downloaded/listened",
                     podcast.title, episode_count);
            continue;
        }

        // Only download the unhandled ones from the recent episodes
        let episodes_to_download: Vec<_> = recent_episodes
            .into_iter()
            .filter(|e| !e.downloaded_on_last_sync && !e.listened_to)
            .collect();

        if episodes_to_download.is_empty() {
            continue;
        }

        println!("Downloading {} episodes for '{}'", episodes_to_download.len(), podcast.title);

        for episode in episodes_to_download {
            let episode_path = podcast_dir.join(episode.safe_filename());

            if episode_path.exists() {
                episode.downloaded_on_last_sync = true;
                continue;
            }

            let progress_bar = multi_progress.add(ProgressBar::new(0));
            progress_bar.set_message(format!("{} - {}", podcast.title, episode.title));

            let client = client.clone();
            let url = episode.enclosure.url.clone();
            let path = episode_path.clone();
            let pub_date = episode.pub_date;

            let task = tokio::spawn(async move {
                let result = download_file(&client, &url, &path, &progress_bar).await;
                if result.is_ok() {
                    // Set file timestamp to publication date
                    let file_time = std::time::SystemTime::try_from(pub_date).unwrap();
                    let _ = filetime::set_file_times(&path,
                        filetime::FileTime::from_system_time(file_time),
                        filetime::FileTime::from_system_time(file_time));
                }
                result
            });

            tasks.push((task, episode));
        }
    }

    if tasks.is_empty() {
        println!("No episodes to download - all recent episodes are already handled");
        return Ok(());
    }

    // Wait for all downloads to complete
    for (task, episode) in tasks {
        match task.await {
            Ok(Ok(())) => {
                episode.downloaded_on_last_sync = true;
                println!("Downloaded: {}", episode.title);
            }
            Ok(Err(e)) => eprintln!("Failed to download {}: {}", episode.title, e),
            Err(e) => eprintln!("Task failed for {}: {}", episode.title, e),
        }
    }

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

async fn create_command(opml: String, output: String, episodes: usize) -> Result<()> {
    println!("Creating podcast database from OPML: {}", opml);

    let mut db = PodderDB::from_opml(&opml).await?;
    let output_path = Path::new(&output);

    fs::create_dir_all(output_path)?;

    println!("Updating RSS feeds...");
    db.update_feeds().await?;

    db.save(output_path)?;
    println!("Database created with {} podcasts", db.podcasts.len());

    if episodes > 0 {
        println!("Downloading {} episodes per podcast...", episodes);
        download_episodes(&mut db, output_path, episodes).await?;
        db.save(output_path)?;
    }

    println!("Setup complete!");
    Ok(())
}

async fn update_command(path: String, download: bool) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = PodderDB::load(base_path)?;

    println!("Updating RSS feeds...");
    db.update_feeds().await?;

    if download {
        println!("Downloading new episodes...");
        download_episodes(&mut db, base_path, 5).await?;
    }

    db.save(base_path)?;
    println!("Update complete!");
    Ok(())
}

async fn download_command(path: String, episodes: usize) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = PodderDB::load(base_path)?;

    println!("Downloading {} episodes per podcast...", episodes);
    download_episodes(&mut db, base_path, episodes).await?;

    db.save(base_path)?;
    println!("Download complete!");
    Ok(())
}

async fn download_one_command(path: String, podcast_idx: usize, episodes: usize) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = PodderDB::load(base_path)?;

    if podcast_idx >= db.podcasts.len() {
        return Err(anyhow::anyhow!("Podcast index {} out of range (0-{})", podcast_idx, db.podcasts.len() - 1));
    }

    println!("Downloading {} episodes for podcast: {}", episodes, db.podcasts[podcast_idx].title);
    download_episodes_filtered(&mut db, base_path, episodes, Some(podcast_idx)).await?;

    db.save(base_path)?;
    println!("Download complete!");
    Ok(())
}

async fn list_command(path: String) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = PodderDB::load(base_path)?;

    println!("Updating RSS feeds...");
    db.update_feeds().await?;

    println!("\nPodcasts:");
    for (i, podcast) in db.podcasts.iter().enumerate() {
        let unheard_count = podcast.episodes.iter()
            .filter(|e| !e.downloaded_on_last_sync && !e.listened_to)
            .count();
        println!("{}: {} ({} unheard episodes)", i, podcast.title, unheard_count);
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

async fn yt_add_command(path: String, url: String, name: String) -> Result<()> {
    let base_path = Path::new(&path);
    let mut db = PodderDB::load(base_path)?;

    db.youtube_playlists.push(YoutubePlaylist {
        title: name.clone(),
        url: url.clone(),
    });

    db.save(base_path)?;
    println!("Added YouTube playlist: {} ({})", name, url);
    Ok(())
}

async fn yt_download_command(path: String) -> Result<()> {
    let base_path = Path::new(&path);
    let db = PodderDB::load(base_path)?;

    if db.youtube_playlists.is_empty() {
        println!("No YouTube playlists configured");
        return Ok(());
    }

    let youtube_dir = base_path.join("youtube_playlists");
    fs::create_dir_all(&youtube_dir)?;

    for playlist in &db.youtube_playlists {
        let playlist_dir = youtube_dir.join(playlist.safe_filename());
        fs::create_dir_all(&playlist_dir)?;

        println!("Downloading playlist: {}", playlist.title);

        let status = tokio::process::Command::new("yt-dlp")
            .arg("-x")
            .arg("--audio-format")
            .arg("mp3")
            .arg("--embed-thumbnail")
            .arg("--add-metadata")
            .arg("-o")
            .arg("%(playlist_index)s - %(title)s.%(ext)s")
            .arg(&playlist.url)
            .arg("--download-archive")
            .arg("downloaded.txt")
            .current_dir(&playlist_dir)
            .status()
            .await?;

        if !status.success() {
            eprintln!("yt-dlp failed for playlist '{}': {}", playlist.title, status);
        } else {
            println!("Successfully downloaded playlist: {}", playlist.title);
        }
    }

    println!("YouTube download complete!");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Create { opml, output, episodes } => {
            create_command(opml, output, episodes).await?;
        }
        Commands::Update { path, download } => {
            update_command(path, download).await?;
        }
        Commands::Download { path, episodes } => {
            download_command(path.clone(), episodes).await?;
        }
        Commands::DownloadOne { path, podcast, episodes } => {
            download_one_command(path.clone(), podcast, episodes).await?;
        }
        Commands::List { path } => {
            list_command(path.clone()).await?;
        }
        Commands::YtAdd { path, url, name } => {
            yt_add_command(path.clone(), url.clone(), name.clone()).await?;
        }
        Commands::YtDownload { path } => {
            yt_download_command(path.clone()).await?;
        }
    }

    Ok(())
}
