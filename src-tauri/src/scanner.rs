use std::path::Path;
use std::process::Command;
use std::fs;
use serde::Deserialize;
use crate::models::Video;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v"];

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: FfprobeFormat,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

pub struct VideoMeta {
    pub duration: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

pub fn get_metadata(path: &str) -> Result<VideoMeta, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            path,
        ])
        .output()
        .map_err(|e| format!("Failed to run ffprobe: {}. Is ffmpeg installed?", e))?;

    if !output.status.success() {
        return Err(format!("ffprobe failed for: {}", path));
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse ffprobe output: {}", e))?;

    let duration = parsed.format.duration
        .and_then(|d| d.parse::<f64>().ok());

    let video_stream = parsed.streams.iter().find(|s| s.codec_type.as_deref() == Some("video"));
    let width = video_stream.and_then(|s| s.width);
    let height = video_stream.and_then(|s| s.height);

    Ok(VideoMeta { duration, width, height })
}

pub fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}

pub fn scan_directory_recursive(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", dir.display()));
    }

    let mut files = Vec::new();
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("Cannot read directory {}: {}", dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Error reading entry: {}", e))?;
        let path = entry.path();

        if path.is_dir() {
            let mut sub = scan_directory_recursive(&path)?;
            files.append(&mut sub);
        } else if is_video_file(&path) {
            files.push(path);
        }
    }

    Ok(files)
}

pub fn build_video(path: &std::path::PathBuf) -> Video {
    let metadata = get_metadata(&path.to_string_lossy()).ok();
    let file_meta = fs::metadata(path).ok();

    Video {
        id: uuid::Uuid::new_v4().to_string(),
        path: path.to_string_lossy().to_string(),
        filename: path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        duration: metadata.as_ref().and_then(|m| m.duration),
        width: metadata.as_ref().and_then(|m| m.width),
        height: metadata.as_ref().and_then(|m| m.height),
        file_size: file_meta.map(|m| m.len() as i64).unwrap_or(0),
        created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_is_video_file() {
        assert!(is_video_file(&PathBuf::from("test.mp4")));
        assert!(is_video_file(&PathBuf::from("test.MKV")));
        assert!(!is_video_file(&PathBuf::from("test.txt")));
        assert!(!is_video_file(&PathBuf::from("test")));
    }

    #[test]
    fn test_scan_empty_directory() {
        let dir = std::env::temp_dir().join(format!("viewman_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let result = scan_directory_recursive(&dir);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());

        fs::remove_dir(&dir).unwrap();
    }
}
