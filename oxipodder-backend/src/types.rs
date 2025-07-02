use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use opml::OPML;
use reqwest::blocking::Client;
use rss::Channel;
use serde::{Deserialize, Serialize};
use serde_json::to_string_pretty;
use url::Url;

use crate::helpers::sanitize_filename;



#[derive(Serialize, Deserialize, Default)]
pub struct PodderDB {
    pub podcasts: Vec<Podcast>
}

#[derive(Serialize, Deserialize)]
pub struct Podcast {
    pub title: String,
    pub description: Option<String>,
    pub xml_url: Url,
    pub html_url: Option<Url>,
    pub auto_download_limit: Option<i32>,
    pub episodes: Vec<Episode>,
    pub last_refreshed: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
pub struct Episode {
    pub guid: String,
    pub title: String,
    pub enclosure: Enclosure,
    pub pub_date: DateTime<Utc>,
    pub downloaded_on_last_sync: bool,
    pub listened_to: bool,

}

#[derive(Serialize, Deserialize)]
pub struct Enclosure {
    pub url: String,
    pub length: i32,
    pub mime_type: String
}

impl Episode {
    pub fn filename(&self) -> String {format!("{}.mp3", sanitize_filename(&self.title))}
}

impl Podcast {
    pub fn filename(&self) -> String {format!("{}", sanitize_filename(&self.title))}
}

impl PodderDB {
    pub fn create_from_opml(opml: OPML) -> Result<PodderDB>{
        let mut db = PodderDB::default();
        for out in &opml.body.outlines.first().unwrap().outlines {
            println!("{out:?}");
            let podcast_result = (|| -> Result<Podcast> {
                Ok(Podcast {
                    title: out.title.clone().context("Missing Title")?,
                    description: out.description.clone(),
                    auto_download_limit: Some(5),
                    xml_url: Url::parse(out.xml_url.clone().context("Missing RSS Url")?.as_str())?,
                    html_url: out.html_url.clone().and_then(|u| Url::parse(&u).ok()),
                    episodes: Vec::new(),
                    last_refreshed: Utc::now()
                })
            })();

            match podcast_result {
                Ok(podcast) => {
                    println!("Successfully added podcast: {}", podcast.title);
                    db.podcasts.push(podcast);
                }
                Err(e) => {
                    eprintln!("Error processing podcast outline: {}", e);
                }
            }
        }

        println!("{}", to_string_pretty(&db).unwrap_or_default());


        Ok(db)
    }

    pub fn update_rss_feeds(&mut self) -> Result<()> {
        let client = Client::new();
        for pod in &mut self.podcasts {
            let content = client.get(pod.xml_url.clone()).send()?.bytes()?;
            let channel = match Channel::read_from(&content[..]) {
                Ok(it) => it,
                Err(e) => {
                    println!("Skipping Podcast: {e}");
                    continue;
                },
            };
            for item in channel.items {
                let guid = item.guid.map(|i| i.value).unwrap_or_default();
                let enclosure = item.enclosure.unwrap_or_default();
                if !pod.episodes.iter().any(|e| e.guid == guid) {
                    pod.episodes.push(Episode {
                        guid,
                        title: item.title.unwrap_or_default(),
                        enclosure: Enclosure {
                            url: enclosure.url,
                            length: enclosure.length.parse().unwrap_or_default(),
                            mime_type: enclosure.mime_type
                        },
                        pub_date: item.pub_date.map(|s| DateTime::parse_from_rfc2822(s.as_str()).unwrap_or_default().into()).unwrap_or_default(),
                        downloaded_on_last_sync: false,
                        listened_to: false
                    });
                }
            }
            pod.last_refreshed = Utc::now();
            pod.episodes.sort_by(|a, b| a.pub_date.cmp(&b.pub_date));
        }

        Ok(())
    }
}
