use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use std::{fs, path::Path};
use url::Url;

use crate::{models::*, utils::create_client};

const DB_FILE_NAME: &str = "podder_db.json";

impl PodderDB {
    pub const PODCAST_DIR: &str = "podcasts";

    pub async fn from_opml(opml_path: &str) -> Result<Self> {
        let content = fs::read_to_string(opml_path)
            .with_context(|| format!("Failed to read OPML file: {}", opml_path))?;

        let opml = opml::OPML::from_str(&content).context("Failed to parse OPML file")?;

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
                                html_url: outline
                                    .html_url
                                    .as_ref()
                                    .and_then(|u| Url::parse(u).ok()),
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

    pub async fn add_feed(&mut self, url: &str) -> Result<()> {
        let parsed_url = Url::parse(url).with_context(|| format!("Invalid URL: {}", url))?;

        if self.podcasts.iter().any(|p| p.xml_url == parsed_url) {
            return Err(anyhow::anyhow!("Podcast with URL '{}' already exists", url));
        }

        let channel = fetch_rss_channel(url).await?;
        let mut podcast = create_podcast_from_channel(channel, parsed_url);

        podcast.episodes.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));

        println!(
            "Added podcast: {} ({} episodes)",
            podcast.title,
            podcast.episodes.len()
        );
        self.podcasts.push(podcast);

        Ok(())
    }

    pub async fn update_feeds(&mut self) -> Result<()> {
        let client = create_client();

        let tasks = self
            .podcasts
            .iter()
            .enumerate()
            .map(|(index, podcast)| {
                let client = client.clone();
                let xml_url = podcast.xml_url.clone();
                let title = podcast.title.clone();

                tokio::spawn(async move {
                    println!("Updating feed for: {}", title);
                    match fetch_channel_content(&client, &xml_url).await {
                        Ok(channel) => Ok((index, channel)),
                        Err(e) => {
                            eprintln!("Failed to update {}: {}", title, e);
                            Err(e)
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        let results = join_all(tasks).await;

        for result in results {
            match result {
                Ok(Ok((index, channel))) => {
                    // Update the podcast at the specific index
                    if let Some(podcast) = self.podcasts.get_mut(index) {
                        update_podcast_episodes(podcast, channel);
                        podcast.last_refreshed = Utc::now();
                    }
                }
                Ok(Err(_)) => {}
                Err(e) => {
                    eprintln!("Task panicked: {}", e);
                }
            }
        }

        Ok(())
    }

    pub fn update_played_statuses(&mut self, base_path: &Path) {
        let all_podcast_path = base_path.join(PodderDB::PODCAST_DIR);
        for podcast in self.podcasts.iter_mut() {
            let podcast_path = all_podcast_path.join(podcast.safe_filename());
            for episode in podcast
                .episodes
                .iter_mut()
                .filter(|e| e.downloaded_on_last_sync)
            {
                let episode_path = podcast_path.join(episode.safe_filename());
                if !episode_path.exists() {
                    episode.listened_to = true;
                    episode.downloaded_on_last_sync = false;
                }
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self).context("Failed to serialize database")?;

        fs::write(path.join(DB_FILE_NAME), content).context("Failed to write database file")?;

        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let db_path = path.join(DB_FILE_NAME);
        let content = fs::read_to_string(&db_path)
            .with_context(|| format!("Failed to read database at {:?}", db_path))?;

        serde_json::from_str(&content).context("Failed to parse database file")
    }
}

async fn fetch_rss_channel(url: &str) -> Result<rss::Channel> {
    let client = create_client();
    println!("Fetching RSS feed from: {}", url);

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch RSS feed from: {}", url))?;

    let content = response
        .bytes()
        .await
        .context("Failed to read RSS feed content")?;

    rss::Channel::read_from(&content[..]).context("Failed to parse RSS feed")
}

async fn fetch_channel_content(client: &reqwest::Client, url: &Url) -> Result<rss::Channel> {
    let response = client.get(url.clone()).send().await?;
    let bytes = response.bytes().await?;
    let content = std::str::from_utf8(&bytes)?;
    if let Some(rss_start) = content.find("<rss") {
        let xml_declaration_end = content.find("?>").map(|pos| pos + 2).unwrap_or(0);
        let rss_content = if rss_start > xml_declaration_end {
            // Keep XML declaration + RSS content
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}",
                &content[rss_start..]
            )
        } else {
            content.to_string()
        };

        Ok(rss::Channel::read_from(rss_content.as_bytes())?)
    } else {
        Err(anyhow!("No RSS tag found"))
    }
}

fn create_podcast_from_channel(channel: rss::Channel, xml_url: Url) -> Podcast {
    let mut podcast = Podcast {
        title: channel.title,
        description: Some(channel.description),
        xml_url,
        html_url: Url::parse(&channel.link).ok(),
        auto_download_limit: Some(5),
        episodes: Vec::new(),
        last_refreshed: Utc::now(),
    };

    for item in channel.items {
        if let Some(episode) = create_episode_from_item(item) {
            podcast.episodes.push(episode);
        }
    }

    podcast
}

fn update_podcast_episodes(podcast: &mut Podcast, channel: rss::Channel) {
    for item in channel.items {
        let guid = item
            .guid
            .as_ref()
            .map(|g| g.value.clone())
            .unwrap_or_else(|| item.title.clone().unwrap_or_default());

        if !podcast.episodes.iter().any(|e| e.guid == guid) {
            if let Some(episode) = create_episode_from_item(item) {
                podcast.episodes.push(episode);
            }
        }
    }

    podcast.episodes.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));
}

fn create_episode_from_item(item: rss::Item) -> Option<Episode> {
    let enclosure = item.enclosure?;
    let guid = item
        .guid
        .map(|g| g.value)
        .unwrap_or_else(|| item.title.clone().unwrap_or_default());

    Some(Episode {
        guid,
        title: item.title.unwrap_or_default(),
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
        downloaded_on_last_sync: false,
        listened_to: false,
    })
}
