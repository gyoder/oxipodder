use anyhow::Result;
use reqwest::blocking::{Client, ClientBuilder};


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


pub fn create_reqwest_client() -> Result<Client> {
    //TODO: make a user agent and headers that doesnt get banned
    Ok(ClientBuilder::new().build()?)
}
