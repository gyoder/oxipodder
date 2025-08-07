use std::path::PathBuf;

use anyhow::Result;
use async_ffmpeg_sidecar::{command::FfmpegCommand, ffprobe::ffprobe_path, paths::ffmpeg_path};
use id3::{Tag, TagLike, Version};
use serde::Deserialize;
use tokio::{io::{AsyncBufReadExt, BufReader}, process::Command};

#[derive(Debug, Deserialize)]
pub struct FfprobeOutput {
    pub chapters: Vec<Chapter>,
    pub format: Option<FormatInfo>,
}

#[derive(Debug, Deserialize)]
pub struct Chapter {
    pub id: u32,
    pub time_base: String,
    pub start: i64,
    pub start_time: String,
    pub end: i64,
    pub end_time: String,
    pub tags: Option<ChapterTags>,
}

#[derive(Debug, Deserialize)]
pub struct ChapterTags {
    pub title: Option<String>,
    pub artist: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FormatInfo {
    pub file_name: Option<String>,
    pub format_name: Option<String>,
    pub duration: Option<String>,
    pub size: Option<String>,
    pub bit_rate: Option<String>,
    pub tags: Option<FormatTags>,
}

#[derive(Debug, Deserialize)]
pub struct FormatTags {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub date: Option<String>,
    pub title: Option<String>,
    pub genre: Option<String>,
}

impl FfprobeOutput {
    pub async fn from_file<T: AsRef<str>>(input_path: T) -> Result<Self> {
        let out = tokio::process::Command::new(ffprobe_path())
            .args([
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_chapters",
                "-show_format",
                input_path.as_ref(),
            ])
            .output()
            .await?;
        let json_output = String::from_utf8(out.stdout)?;
        let ffprobe_output: FfprobeOutput = serde_json::from_str(&json_output)?;
        Ok(ffprobe_output)
    }


    pub fn duration_seconds(&self) -> Option<f64> {
        self.format
            .as_ref()
            .and_then(|f| f.duration.as_ref())
            .and_then(|d| d.parse().ok())
    }


    pub fn duration_formatted(&self) -> Option<String> {
        self.duration_seconds().map(|seconds| {
            let total_seconds = seconds as u64;
            let hours = total_seconds / 3600;
            let minutes = (total_seconds % 3600) / 60;
            let secs = total_seconds % 60;

            if hours > 0 {
                format!("{:02}:{:02}:{:02}", hours, minutes, secs)
            } else {
                format!("{:02}:{:02}", minutes, secs)
            }
        })
    }


    pub fn bitrate(&self) -> Option<u64> {
        self.format
            .as_ref()
            .and_then(|f| f.bit_rate.as_ref())
            .and_then(|b| b.parse().ok())
    }


    pub fn file_size(&self) -> Option<u64> {
        self.format
            .as_ref()
            .and_then(|f| f.size.as_ref())
            .and_then(|s| s.parse().ok())
    }


    pub fn audio_info(&self) -> AudioInfo {
        AudioInfo {
            duration_seconds: self.duration_seconds(),
            duration_formatted: self.duration_formatted(),
            bitrate: self.bitrate(),
            file_size: self.file_size(),
            format_name: self.format.as_ref().and_then(|f| f.format_name.clone()),
            title: self.format.as_ref().and_then(|f| f.tags.as_ref().and_then(|t| t.title.clone())),
            artist: self.format.as_ref().and_then(|f| f.tags.as_ref().and_then(|t| t.artist.clone())),
            album: self.format.as_ref().and_then(|f| f.tags.as_ref().and_then(|t| t.album.clone())),
            genre: self.format.as_ref().and_then(|f| f.tags.as_ref().and_then(|t| t.genre.clone())),
            chapter_count: self.chapters.len(),
        }
    }

    pub fn generate_cuesheet(&self) -> Result<String> {
        let mut cuesheet = String::new();


        let default_name = "audio.mp3".to_string();
        let filename = self.format
            .as_ref()
            .and_then(|f| f.file_name.as_ref())
            .unwrap_or(&default_name);

        cuesheet.push_str(&format!("FILE \"{}\" MP3\n", filename));


        if let Some(format) = &self.format {
            if let Some(tags) = &format.tags {
                if let Some(title) = &tags.title {
                    cuesheet.push_str(&format!("TITLE \"{}\"\n", title));
                }
                if let Some(artist) = &tags.artist {
                    cuesheet.push_str(&format!("PERFORMER \"{}\"\n", artist));
                }
            }
        }

        for (i, chapter) in self.chapters.iter().enumerate() {
            let track_number = i + 1;
            let default_track_title = format!("Track {}", track_number);
            let title = chapter
                .tags
                .as_ref()
                .and_then(|t| t.title.as_ref())
                .unwrap_or(&default_track_title);

            let start_seconds: f64 = chapter.start_time.parse()?;
            let cue_time = seconds_to_cue_time(start_seconds);

            cuesheet.push_str(&format!("  TRACK {:02} AUDIO\n", track_number));
            cuesheet.push_str(&format!("    TITLE \"{}\"\n", title));

            if let Some(tags) = &chapter.tags {
                if let Some(artist) = &tags.artist {
                    cuesheet.push_str(&format!("    PERFORMER \"{}\"\n", artist));
                }
            }

            cuesheet.push_str(&format!("    INDEX 01 {}\n", cue_time));
        }

        Ok(cuesheet)
    }
}

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub duration_seconds: Option<f64>,
    pub duration_formatted: Option<String>,
    pub bitrate: Option<u64>,
    pub file_size: Option<u64>,
    pub format_name: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub chapter_count: usize,
}

impl AudioInfo {

    pub fn bitrate_kbps(&self) -> Option<u64> {
        self.bitrate.map(|b| b / 1000)
    }


    pub fn file_size_mb(&self) -> Option<f64> {
        self.file_size.map(|s| s as f64 / 1_048_576.0)
    }


    pub fn summary(&self) -> String {
        let mut parts = Vec::new();

        if let Some(duration) = &self.duration_formatted {
            parts.push(format!("Duration: {}", duration));
        }

        if let Some(bitrate) = self.bitrate_kbps() {
            parts.push(format!("Bitrate: {}kbps", bitrate));
        }

        if let Some(size) = self.file_size_mb() {
            parts.push(format!("Size: {:.1}MB", size));
        }

        if self.chapter_count > 0 {
            parts.push(format!("Chapters: {}", self.chapter_count));
        }

        if parts.is_empty() {
            "No audio info available".to_string()
        } else {
            parts.join(", ")
        }
    }
}

pub async fn replaygain_analysis(input: &PathBuf) -> Result<(Option<f32>, Option<f32>)> {
    let mut replaygain_cmd = Command::new(ffmpeg_path())
        .args([
            "-i",
            input.to_str().unwrap(),
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

    Ok((track_gain, track_peak))
}

pub fn write_replay_gain(file: &PathBuf, track_gain: Option<f32>, track_peak: Option<f32>) -> Result<()> {
    let path = file.to_str().unwrap();
    let mut tag = Tag::read_from_path(path)?;

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

    tag.write_to_path(path, Version::Id3v23)?;
    Ok(())
}

pub enum TranscodeOptions {
    Compress,
    Copy,
    Default,
}

pub fn transcode_command(input: &PathBuf, output: &PathBuf, opts: TranscodeOptions) -> FfmpegCommand {
    let mut cmd = FfmpegCommand::new();

    match opts {
        TranscodeOptions::Compress => {
            cmd.input(input.to_str().unwrap())
                .args(["-codec:a", "libmp3lame"])
                .args(["-q:a", "6"])
                .no_video()
                .output(output.to_str().unwrap());
            cmd
        },
        TranscodeOptions::Copy => {
            cmd.input(input.to_str().unwrap())
                .codec_audio("copy")
                .no_video()
                .output(output.to_str().unwrap());
            cmd
        },
        TranscodeOptions::Default => {
            cmd.input(input.to_str().unwrap())
                .args(["-codec:a", "libmp3lame"])
                .args(["-q:a", "0"])
                .no_video()
                .output(output.to_str().unwrap());
            cmd
        }
    }
}

fn seconds_to_cue_time(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let minutes = total_seconds / 60;
    let secs = total_seconds % 60;
    let frames = ((seconds.fract() * 75.0).round() as u64).min(74);
    format!("{:02}:{:02}:{:02}", minutes, secs, frames)
}

