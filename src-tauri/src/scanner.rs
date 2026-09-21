use std::path::Path;
use std::process::Command;
use std::fs;
use serde::Deserialize;
use crate::models::Video;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v"];

/// 启动 ffprobe/ffmpeg 这类控制台程序。Windows 上必须带 CREATE_NO_WINDOW，
/// 否则 release 版（GUI 子系统）每调用一次就会闪现一个黑色控制台窗口。
pub(crate) fn hidden_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

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
    let output = hidden_command("ffprobe")
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
    // 转码中断可能留下 `.viewman-h264-*.tmp.mp4`，不能当视频入库
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(".viewman-"))
    {
        return false;
    }
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
            // 回收站/系统目录里的 $R*.mp4 是已删除文件的副本，不能重新入库
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_lowercase();
            if dir_name == "$recycle.bin" || dir_name == "system volume information" {
                continue;
            }
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
        thumbnail_path: None,
    }
}

/// 缩略图目标宽度；高度按原始比例自适应（-2 保证偶数）
const THUMBNAIL_WIDTH: i32 = 480;

/// 抽帧位置：取 10% 处（跳过片头黑场/台标），最多 10 秒；时长未知则取首帧
pub fn thumbnail_target_time(duration: Option<f64>) -> f64 {
    match duration {
        Some(d) if d.is_finite() && d > 0.0 => (d * 0.1).min(10.0),
        _ => 0.0,
    }
}

pub fn ffmpeg_available() -> bool {
    hidden_command("ffmpeg").arg("-version").output().is_ok()
}

/// 读取视频流编码名（h264/hevc/...），读不出时返回 None
pub fn video_codec_name(video_path: &str) -> Option<String> {
    let output = hidden_command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            video_path,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// 转码为 H.264 + AAC 写入 out_path，供 WebView2 解不了 HEVC 时内置播放使用
pub fn transcode_to_h264(video_path: &str, out_path: &Path) -> Result<(), String> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建转码目录失败: {}", e))?;
    }
    let output = hidden_command("ffmpeg")
        .args([
            "-y",
            "-i",
            video_path,
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
        ])
        .arg(out_path)
        .output()
        .map_err(|e| format!("无法运行 ffmpeg: {}", e))?;
    if !output.status.success() {
        let _ = fs::remove_file(out_path);
        let detail = String::from_utf8_lossy(&output.stderr)
            .lines()
            .last()
            .unwrap_or("ffmpeg 执行失败")
            .to_string();
        return Err(detail);
    }
    match fs::metadata(out_path) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => Err("ffmpeg 未产生有效输出".to_string()),
    }
}

/// 用 mpdecimate 去除重复帧后统计剩余画面数，判断是否"近似静图"。无法解码时返回 None
pub fn unique_frame_count(video_path: &str) -> Option<usize> {
    let output = hidden_command("ffmpeg")
        .args([
            "-hide_banner",
            "-i",
            video_path,
            "-vf",
            "mpdecimate",
            "-f",
            "null",
            "-",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let mut count = None;
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("frame=") {
            if let Some(num) = rest.trim().split_whitespace().next() {
                if let Ok(n) = num.parse::<usize>() {
                    count = Some(n);
                }
            }
        }
    }
    count
}

/// 抽取原视频的首帧（保持原始分辨率）写入 out_path，用于把 ≤1 秒的"静图视频"转成图片
pub fn extract_full_frame(video_path: &str, out_path: &Path) -> Result<(), String> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
    }
    let output = hidden_command("ffmpeg")
        .args([
            "-y",
            "-i",
            video_path,
            "-frames:v",
            "1",
            "-q:v",
            "2",
            &out_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("无法运行 ffmpeg: {}", e))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .lines()
            .last()
            .unwrap_or("ffmpeg 执行失败")
            .to_string();
        return Err(detail);
    }
    match fs::metadata(out_path) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => Err("ffmpeg 未产生有效帧".to_string()),
    }
}

/// 用 ffmpeg 抽取一帧写入 out_path。先尝试 10% 位置，失败则回退到首帧。
pub fn extract_thumbnail(
    video_path: &str,
    out_path: &Path,
    duration: Option<f64>,
) -> Result<(), String> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建缩略图目录失败: {}", e))?;
    }

    let attempt = |seconds: f64| -> Result<(), String> {
        let output = hidden_command("ffmpeg")
            .args([
                "-y",
                "-ss",
                &seconds.to_string(),
                "-i",
                video_path,
                "-frames:v",
                "1",
                "-vf",
                &format!("scale={}:-2", THUMBNAIL_WIDTH),
                "-q:v",
                "4",
                &out_path.to_string_lossy(),
            ])
            .output()
            .map_err(|e| format!("无法运行 ffmpeg: {}", e))?;

        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("ffmpeg 执行失败")
                .to_string();
            return Err(detail);
        }

        // seek 点无帧时 ffmpeg 仍可能以 0 退出，但不会写出有效文件
        match fs::metadata(out_path) {
            Ok(m) if m.len() > 0 => Ok(()),
            _ => Err("ffmpeg 未产生有效帧".to_string()),
        }
    };

    let seek = thumbnail_target_time(duration);
    if seek > 0.0 {
        attempt(seek).or_else(|_| attempt(0.0))
    } else {
        attempt(0.0)
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
        assert!(!is_video_file(&PathBuf::from(".viewman-h264-abc.tmp.mp4")));
    }

    #[test]
    fn test_thumbnail_target_time() {
        // 10% 但最多 10 秒
        assert_eq!(thumbnail_target_time(Some(3600.0)), 10.0);
        assert!((thumbnail_target_time(Some(60.0)) - 6.0).abs() < 0.001);
        assert!((thumbnail_target_time(Some(120.0)) - 10.0).abs() < 0.001);
        // 时长未知/非法时退回首帧
        assert_eq!(thumbnail_target_time(None), 0.0);
        assert_eq!(thumbnail_target_time(Some(0.0)), 0.0);
        assert_eq!(thumbnail_target_time(Some(f64::NAN)), 0.0);
    }

    #[test]
    fn test_extract_thumbnail_from_generated_video() {
        if !ffmpeg_available() {
            eprintln!("跳过：本机未安装 ffmpeg");
            return;
        }
        let dir = std::env::temp_dir().join(format!("viewman_thumb_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        // 用 ffmpeg 造一段 2 秒测试视频，避免依赖任何外部素材
        let video = dir.join("sample.mp4");
        let generated = Command::new("ffmpeg")
            .args([
                "-y",
                "-f", "lavfi",
                "-i", "testsrc=size=320x240:rate=10:duration=2",
                "-pix_fmt", "yuv420p",
                &video.to_string_lossy(),
            ])
            .output()
            .unwrap();
        assert!(generated.status.success(), "生成测试视频失败");

        let out = dir.join("thumb.jpg");
        extract_thumbnail(&video.to_string_lossy(), &out, Some(2.0)).unwrap();
        assert!(fs::metadata(&out).unwrap().len() > 0, "缩略图应为非空文件");

        fs::remove_dir_all(&dir).unwrap();
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
