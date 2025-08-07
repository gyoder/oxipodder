use anyhow::Result;
use clap::Parser;

mod cli;
mod database;
mod ffmpeg;
mod models;
mod utils;

use cli::*;

#[derive(Parser)]
#[command(name = "oxipodder")]
#[command(about = "A fast and simple podcast downloader")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Create {
            opml,
            output,
            episodes,
        } => create_command(opml, output, episodes).await,
        Commands::Add { path, url } => add_command(path, url).await,
        Commands::Update { path, download } => update_command(path, download).await,
        Commands::Download { path, episodes } => download_command(path, episodes).await,
        Commands::DownloadOne {
            path,
            podcast,
            episodes,
        } => download_one_command(path, podcast, episodes).await,
        Commands::List { path } => list_command(path).await,
        Commands::YtAdd { path, url, name } => yt_add_command(path, url, name).await,
        Commands::YtDownload { path } => yt_download_command(path).await,
    }
}
