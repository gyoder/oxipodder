use std::{collections::HashMap, thread::JoinHandle};

use anyhow::Result;
use crossbeam::channel::Receiver;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use oxipodder_backend::downloader::DownloadMessage;


fn create_task_text(total_size: u64, completed: u64, name: &str) -> String {
    let com_mb = completed as f64 / 1048576.0;
    let tot_mb = total_size as f64 / 1048576.0;
    format!("{com_mb:.1} / {tot_mb:.1} MB - {name}")
}

pub fn create_download_view(rx: Receiver<DownloadMessage>, handles: Vec<JoinHandle<()>>, display_texts: Vec<String>) -> Result<()> {
    let mut mb = MultiProgress::new();
    let mut bars: HashMap<u32, ProgressBar> = HashMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            DownloadMessage::Started(dp) => {
                let pb: ProgressBar = ProgressBar::new(100);
                pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{bar:40.cyan/blue}] {pos:>7}/{len:7} {msg}")
                    .unwrap()
                    .progress_chars("#>-")
                );
                pb.set_message(create_task_text(dp.total_size, dp.completed, &*display_texts.get(dp.id as usize).unwrap_or(&"".to_string())));
                bars.insert(dp.id, mb.add(pb));
            },
            DownloadMessage::Incremental(dp) => {
                let pb = bars.get(&dp.id).unwrap();
                let percent: u64 = 100 * dp.completed / dp.total_size;
                pb.set_position(percent);
                pb.set_message(create_task_text(dp.total_size, dp.completed, &*display_texts.get(dp.id as usize).unwrap_or(&"".to_string())));
            },
            DownloadMessage::Completed(dp) => {
                let pb = bars.get(&dp.id).unwrap();
                pb.finish_with_message(format!("Downloaded {}", *display_texts.get(dp.id as usize).unwrap_or(&"".to_string())));
            },
            DownloadMessage::Failed(_) => todo!(),
            DownloadMessage::ThreadTerminated => {},
        };
    }
    Ok(())
}
