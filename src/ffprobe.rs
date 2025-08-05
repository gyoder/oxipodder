use anyhow::Result;
use async_ffmpeg_sidecar::ffprobe::ffprobe_path;
use serde::Deserialize;

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
    pub tags: Option<FormatTags>,
}

#[derive(Debug, Deserialize)]
pub struct FormatTags {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub date: Option<String>,
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

    pub fn generate_cuesheet(&self) -> Result<String> {
        let mut cuesheet = String::new();

        cuesheet.push_str(&format!(
            "FILE \"{}\" MP3\n",
            self.format
                .as_ref()
                .and_then(|f| f.format_name.as_ref())
                .unwrap_or(&"audio.mp3".to_string())
        ));

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

fn seconds_to_cue_time(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let minutes = total_seconds / 60;
    let secs = total_seconds % 60;
    let frames = ((seconds.fract() * 75.0).round() as u64).min(74);

    format!("{:02}:{:02}:{:02}", minutes, secs, frames)
}
