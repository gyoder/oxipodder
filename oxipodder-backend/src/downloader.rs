use std::{any::Any, fs::{remove_file, File}, io::{Read, Write}, path::{Path, PathBuf}, process::Stdio, str::FromStr, sync::{mpsc, Arc}, thread::{self, JoinHandle}};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use crossbeam::{channel::{unbounded, Receiver}, queue::ArrayQueue};
use filetime::{set_file_atime, set_file_times, FileTime};
use reqwest::blocking::Response;
use url::Url;

use crate::helpers::create_reqwest_client;


pub struct DownloadProgress {
    pub id: u32,
    pub total_size: u64,
    pub completed: u64,
}

impl DownloadProgress {
    pub fn new(id: u32, total_size: u64, completed: u64) -> Self {
        Self { id, total_size, completed }
    }
}


pub enum DownloadMessage {
    Started(DownloadProgress),
    Incremental(DownloadProgress),
    Completed(DownloadProgress),
    Failed(String),
    ThreadTerminated
}

pub struct DownloadQueueElement {
    pub name: String,
    pub id: u32,
    pub url: Url,
    pub location: PathBuf,
    pub pub_date: DateTime<Utc>
}

pub fn create_downloader(download_list: Vec<DownloadQueueElement>, threads: i32) -> Result<(Receiver<DownloadMessage>, Vec<JoinHandle<()>>)> {
    let download_queue: Arc<ArrayQueue<DownloadQueueElement>> = Arc::new(ArrayQueue::new(download_list.len()));
    for e in download_list.into_iter() {
        download_queue.push(e).map_err(|e| anyhow!("Failed to create download queue"))?;
    }

    let (tx, rx) = unbounded::<DownloadMessage>();
    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    for _ in 0..threads {
        let download_queue = download_queue.clone();
        let tx = tx.clone();
        let handle = thread::spawn(move || {
            let client = create_reqwest_client().unwrap();
            while let Some(e) = download_queue.pop() {
                let mut response: Response = match client.get(e.url.clone()).send() {
                    Ok(res) => res,
                    Err(e) => {
                        tx.send(DownloadMessage::Failed(e.to_string())).unwrap(); //TODO: handle
                        continue;
                    },
                };

                let is_mp3 = e.url.path().ends_with("mp3");

                let total_size = response.content_length().unwrap_or_default();
                let mut completed: u64 = 0;
                let dl_path = if is_mp3 {e.location.clone()} else {PathBuf::from_str(&format!("/tmp/oxi_{}", e.id)).unwrap()};
                let mut file = File::create(&dl_path).unwrap();
                let mut buf = [0; 8192];

                tx.send(DownloadMessage::Started(DownloadProgress::new(e.id, total_size, completed))).unwrap();
                let i = 0;
                while let Ok(bytes_read) = response.read(&mut buf) {
                    if bytes_read == 0 {break;}

                    file.write_all(&buf[..bytes_read]).unwrap();
                    completed += bytes_read as u64;
                    if i % 10 == 0 {
                        tx.send(DownloadMessage::Incremental(DownloadProgress::new(e.id, total_size, completed))).unwrap();
                    }
                }

                if !is_mp3 {
                    let status = std::process::Command::new("ffmpeg")
                        .args(&[
                            "-y",
                            "-i", dl_path.to_str().unwrap_or_default(),
                            e.location.to_str().unwrap_or_default(),
                        ])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();

                    match status {
                        Ok(s) if s.success() => {},
                        _ => {
                            tx.send(DownloadMessage::Failed("Failed to Transcode".to_string())).unwrap();
                        }
                    }
                    let _ = remove_file(dl_path);
                }

                let unix = FileTime::from_unix_time(e.pub_date.timestamp(), 0);
                set_file_times(e.location, unix, unix).unwrap();

                tx.send(DownloadMessage::Completed(DownloadProgress::new(e.id, total_size, completed))).unwrap();

            }
            tx.send(DownloadMessage::ThreadTerminated).unwrap(); // just to get the feel of it
        });
        handles.push(handle);
    }

    Ok((rx, handles))
}
