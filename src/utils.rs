use reqwest::{Client, ClientBuilder};

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
