use crate::utils::sanitize_filename;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize, Default)]
pub struct PodderDB {
    #[serde(default)]
    pub podcasts: Vec<Podcast>,
    #[serde(default)]
    pub youtube_playlists: Vec<YoutubePlaylist>,
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
    #[serde(alias = "downloaded")]
    pub downloaded_on_last_sync: bool,
    pub listened_to: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Enclosure {
    pub url: String,
    pub length: i32,
    pub mime_type: String,
}

#[derive(Serialize, Deserialize)]
pub struct YoutubePlaylist {
    pub title: String,
    pub url: String,
}

impl Podcast {
    pub fn safe_filename(&self) -> String {
        sanitize_filename(&self.title)
    }
}

impl Episode {
    pub fn safe_filename(&self) -> String {
        format!("{}.mp3", sanitize_filename(&self.title))
    }
}

impl YoutubePlaylist {
    pub fn safe_filename(&self) -> String {
        sanitize_filename(&self.title)
    }
}
