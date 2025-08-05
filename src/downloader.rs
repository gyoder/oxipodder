use anyhow::{Result, anyhow};
use async_ffmpeg_sidecar::command::FfmpegCommand;
use async_ffmpeg_sidecar::paths::ffmpeg_path;
use async_tempfile::TempFile;
use futures_util::StreamExt;
use id3::{Tag, TagLike, Version};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::{fs, path::Path, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::ffprobe::FfprobeOutput;
use crate::models::*;
use crate::utils::create_client;

pub async fn download_episodes(
    db: &mut PodderDB,
    base_path: &Path,
    episode_count: usize,
) -> Result<()> {
    download_episodes_filtered(db, base_path, episode_count, None).await
}

pub async fn download_episodes_filtered(
    db: &mut PodderDB,
    base_path: &Path,
    episode_count: usize,
    podcast_filter: Option<usize>,
) -> Result<()> {
    let podcasts_dir = base_path.join(PodderDB::PODCAST_DIR);
    fs::create_dir_all(&podcasts_dir)?;

    let client = create_client();
    let multi_progress = Arc::new(MultiProgress::new());
    let mut tasks = Vec::new();

    for (i, podcast) in db.podcasts.iter_mut().enumerate() {
        if let Some(filter_idx) = podcast_filter {
            if i != filter_idx {
                continue;
            }
        }

        let podcast_dir = podcasts_dir.join(podcast.safe_filename());
        fs::create_dir_all(&podcast_dir)?;

        podcast.episodes.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));

        let podcast_title = podcast.title.clone();

        let episodes_to_download = get_episodes_to_download(podcast, episode_count);

        if episodes_to_download.is_empty() {
            continue;
        }

        println!(
            "Downloading {} episodes for '{}'",
            episodes_to_download.len(),
            podcast_title
        );

        for episode in episodes_to_download {
            let episode_path = podcast_dir.join(episode.safe_filename());

            if episode_path.exists() {
                episode.downloaded_on_last_sync = true;
                continue;
            }

            let progress_bar = multi_progress.add(ProgressBar::new(0));
            progress_bar.set_message(format!("{} - {}", podcast_title, episode.title));

            let task = create_download_task(client.clone(), episode, episode_path, progress_bar);
            tasks.push(task);
        }
    }

    if tasks.is_empty() {
        println!("No episodes to download - all recent episodes are already handled");
        return Ok(());
    }

    execute_download_tasks(tasks).await;
    Ok(())
}

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    progress_bar: &ProgressBar,
) -> Result<TempFile> {
    let response = client.get(url).send().await?;
    let total_size = response.content_length().unwrap_or(0);

    progress_bar.set_length(total_size);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut file = TempFile::new().await?;
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        progress_bar.set_position(downloaded);
    }
    progress_bar.set_position(downloaded);
    progress_bar.set_message("Writing to File");

    file.flush().await?;
    Ok(file)
}

async fn transcode(
    input_file: TempFile,
    output_path: &Path,
    progress_bar: &ProgressBar,
) -> Result<()> {
    progress_bar.set_position(progress_bar.length().unwrap_or_default());
    let input_path = input_file.file_path().to_string_lossy();
    let output_path_str = output_path.to_string_lossy();
    let ffprobe_out = FfprobeOutput::from_file(&input_path).await?;

    // ReplayGain Calcs
    let mut replaygain_cmd = Command::new(ffmpeg_path())
        .args([
            "-i",
            input_path.as_ref(),
            "-af",
            "replaygain",
            "-f",
            "null",
            "-",
        ])
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let stderr = replaygain_cmd.stderr.take().unwrap();
    let reader = BufReader::new(stderr);
    let mut lines = reader.lines();

    let mut track_gain: Option<f32> = None;
    let mut track_peak: Option<f32> = None;

    while let Some(line) = lines.next_line().await? {
        if line.contains("track_gain =") {
            if let Some(num_str) = line
                .split("track_gain =")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
            {
                if let Ok(num) = num_str.parse::<f32>() {
                    track_gain = Some(num);
                }
            }
        } else if line.contains("track_peak =") {
            if let Some(num_str) = line
                .split("track_peak =")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
            {
                if let Ok(num) = num_str.parse::<f32>() {
                    track_peak = Some(num);
                }
            }
        }
    }

    // Transcode/Copy
    if ffprobe_out
        .format
        .and_then(|f| f.format_name)
        .is_some_and(|f| &f == "mp3")
    {
        tokio::fs::copy(input_path.as_ref(), output_path_str.as_ref()).await?;
    } else if !FfmpegCommand::new()
        .input(input_path)
        .no_video()
        .output(output_path_str.as_ref())
        .spawn()?
        .wait()
        .await?
        .success()
    {
        return Err(anyhow!("FFMpeg Transcode Failed"));
    }

    // Write ReplayGain
    let mut tag = Tag::read_from_path(output_path_str.as_ref())?;

    if let Some(gain) = track_gain {
        tag.add_frame(id3::Frame::with_content(
            "TXXX",
            id3::Content::ExtendedText(id3::frame::ExtendedText {
                description: "REPLAYGAIN_TRACK_GAIN".to_string(),
                value: format!("{:.2} dB", gain),
            }),
        ));
    }
    if let Some(peak) = track_peak {
        tag.add_frame(id3::Frame::with_content(
            "TXXX",
            id3::Content::ExtendedText(id3::frame::ExtendedText {
                description: "REPLAYGAIN_TRACK_PEAK".to_string(),
                value: format!("{:.6}", peak),
            }),
        ));
    }

    tag.write_to_path(output_path_str.as_ref(), Version::Id3v23)?;
    Ok(())
}

fn get_episodes_to_download(podcast: &mut Podcast, episode_count: usize) -> Vec<&mut Episode> {
    podcast
        .episodes
        .iter_mut()
        .take(episode_count)
        .filter(|e| !e.downloaded_on_last_sync && !e.listened_to)
        .collect()
}

fn create_download_task(
    client: reqwest::Client,
    episode: &mut Episode,
    episode_path: std::path::PathBuf,
    progress_bar: ProgressBar,
) -> (tokio::task::JoinHandle<Result<()>>, &mut Episode) {
    let url = episode.enclosure.url.clone();
    let pub_date = episode.pub_date;

    let task = tokio::spawn(async move {
        let result = download_file(&client, &url, &progress_bar).await;
        let Ok(file) = result else {
            return Err(anyhow!("Failed to download file"));
        };
        let result = transcode(file, &episode_path, &progress_bar).await;
        let Ok(()) = result else {
            return Err(anyhow!("Failed to transcode file"));
        };
        let file_time = std::time::SystemTime::from(pub_date);
        let _ = filetime::set_file_times(
            &episode_path,
            filetime::FileTime::from_system_time(file_time),
            filetime::FileTime::from_system_time(file_time),
        );
        Ok(())
    });

    (task, episode)
}

async fn execute_download_tasks(tasks: Vec<(tokio::task::JoinHandle<Result<()>>, &mut Episode)>) {
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
}
