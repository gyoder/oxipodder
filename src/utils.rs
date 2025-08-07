use reqwest::{Client, ClientBuilder};
use anyhow::{Context, Result};
use url::Url;

pub fn create_client() -> Client {
    ClientBuilder::new()
        .user_agent(format!(
            "Mozilla/5.0 (compatible; {}/{})",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .unwrap()
}

pub fn sanitize_filename(name: &str) -> String {
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

pub async fn fetch_rss_channel(url: &Url) -> Result<rss::Channel> {
    let client = create_client();
    println!("Fetching RSS feed from: {}", url);

    let response = client
        .get(url.clone())
        .send()
        .await
        .with_context(|| format!("Failed to fetch RSS feed from: {}", url))?;

    let content = response
        .bytes()
        .await
        .context("Failed to read RSS feed content")?;

    rss::Channel::read_from(&content[..]).context("Failed to parse RSS feed")
}
