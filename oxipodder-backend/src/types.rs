use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use opml::OPML;
use reqwest::blocking::Client;
use rss::Channel;
use serde::{Deserialize, Serialize};
use url::Url;



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

impl PodderDB {
    pub fn create_from_opml(opml: OPML) -> Result<PodderDB>{
        let mut db = PodderDB::default();
        for out in opml.body.outlines {
            db.podcasts.push(Podcast {
                title: out.title.context("Missing Title")?,
                description: out.description,
                auto_download_limit: Some(5),
                xml_url: Url::parse(out.xml_url.context("Missing RSS Url")?.as_str())?,
                html_url: out.html_url.map(|u| Url::parse(&u).ok()).unwrap_or_default(),
                episodes: Vec::new(),
                last_refreshed: DateTime::default()
            });
        }


        Ok(db)
    }

    pub fn update_rss_feeds(&mut self) -> Result<()> {
        let client = Client::new();
        for pod in &mut self.podcasts {
            let content = client.get(pod.xml_url.clone()).send()?.bytes()?;
            let channel = Channel::read_from(&content[..])?;
            for item in channel.items {
                let guid = item.guid.map(|i| i.value).unwrap_or_default();
                let enclosure = item.enclosure.unwrap_or_default();
                if pod.episodes.iter().any(|e| e.guid == guid) {
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
