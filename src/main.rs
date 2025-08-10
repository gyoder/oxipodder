use std::{env::set_current_dir, path::PathBuf, str::FromStr};

use anyhow::Result;
use clap::Parser;
use sea_orm::{
    prelude::*, ActiveModelTrait, ConnectOptions, Database, DatabaseConnection
};


mod cli;
mod ffmpeg;
mod models;
mod utils;

use cli::*;
use tokio::sync::OnceCell;

use crate::models::{episode, podcast, settings, youtube_playlist, youtube_video};

#[derive(Parser)]
#[command(name = "oxipodder")]
#[command(about = "A fast and simple podcast downloader")]
struct Cli {
    #[arg(short = 'P', long, default_value = ".")]
    path: String,
    #[arg(short = 'C', long, default_value = "false")]
    create_if_none: bool,
    #[command(subcommand)]
    command: Commands,
}

static DB: OnceCell<DatabaseConnection> = OnceCell::const_new();

pub async fn init_db(create_if_none: bool) -> Result<()> {
    let mut opt = ConnectOptions::new(format!("sqlite:./podder_db.sql{}", if create_if_none {"?mode=rwc"} else {""}));
    opt.max_connections(100)
        .min_connections(5)
        .sqlx_logging(true);
    let db = Database::connect(opt).await?;

    // Create tables with IF NOT EXISTS
    let schema = sea_orm::Schema::new(sea_orm::DatabaseBackend::Sqlite);

    db.execute(db.get_database_backend().build(
        schema.create_table_from_entity(podcast::Entity).if_not_exists()
    )).await?;

    db.execute(db.get_database_backend().build(
        schema.create_table_from_entity(episode::Entity).if_not_exists()
    )).await?;

    db.execute(db.get_database_backend().build(
        schema.create_table_from_entity(youtube_playlist::Entity).if_not_exists()
    )).await?;

    db.execute(db.get_database_backend().build(
        schema.create_table_from_entity(youtube_video::Entity).if_not_exists()
    )).await?;

    db.execute(db.get_database_backend().build(
        schema.create_table_from_entity(settings::Entity).if_not_exists()
    )).await?;

    DB.set(db).map_err(|_| anyhow::anyhow!("Failed to set global DB"))?;
    Ok(())
}

pub fn db() -> &'static DatabaseConnection {
    DB.get().expect("Database not initialized")
}

#[tokio::main]
async fn main() -> Result<()> {
    // tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    set_current_dir(PathBuf::from_str(&cli.path).unwrap()).unwrap();
    init_db(cli.create_if_none).await.expect("Failed to open and init database");

    match cli.command {
        Commands::Create {
            opml,
        } => create_command(opml).await,
        Commands::Add { url } => add_command(url).await,
        Commands::Update { download } => update_command(download).await,
        Commands::Download { episodes, no_episode_limit, download_listened } => download_command(episodes, no_episode_limit, download_listened).await,
        Commands::DownloadOne { podcast_number, episodes, no_episode_limit, download_listened } => download_one_command(podcast_number, episodes, no_episode_limit, download_listened).await,
        Commands::List => list_command().await,
        Commands::YtAdd { url, name } => yt_add_command(url, name).await,
        Commands::YtDownload => yt_download_command().await,
        Commands::ChangeSettings { settings } => settings_command(settings).await,
        Commands::ChangePodcastSettings { podcast_number, settings } => podcast_settings_command(podcast_number, settings).await,
    }
}
