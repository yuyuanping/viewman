use std::path::Path;
use std::process::Command;
use std::fs;
use serde::Deserialize;

use crate::models::{Image, Video};

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v"];
/// HEIC/HEIF 刻意不入库：WebView2 无法解码，收进来只会显示成裂图
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "avif"];

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

/// image2 解封装器会把文件名里的 `%` 当序号模板（QQ/微信存下来的图常见
/// `$xx%yy.jpg` 这类名字），不关掉序列匹配就报 "No such file or directory"，
/// 尺寸、封面、pHash 全都读不到。
///
/// 条件收得很紧，两个都不能省：
/// - 名字里没有 `%` 时不能加——那时 ffmpeg 走的不是 image2 序列路径，
///   这个私有选项会导致 "Option not found"，连输入都打不开（实测）；
/// - 视频不能加——mov/matroska 不认识该选项，同样 "Option not found"。
fn image_input_opts(path: &str) -> &'static [&'static str] {
    if path.contains('%') && is_image_file(Path::new(path)) {
        &["-pattern_type", "none"]
    } else {
        &[]
    }
}

pub fn get_metadata(path: &str) -> Result<VideoMeta, String> {
    let output = hidden_command("ffprobe")
        .args(image_input_opts(path))
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

/// 转码中断可能留下 `.viewman-h264-*.tmp.mp4`，不能当视频入库
fn is_temp_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(".viewman-"))
}

fn has_known_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .is_some_and(|e| extensions.contains(&e.as_str()))
}

pub fn is_video_file(path: &Path) -> bool {
    !is_temp_artifact(path) && has_known_extension(path, VIDEO_EXTENSIONS)
}

pub fn is_image_file(path: &Path) -> bool {
    !is_temp_artifact(path) && has_known_extension(path, IMAGE_EXTENSIONS)
}

/// 一次全盘走查的产物。`files` 之外还要带出"哪里没走成"：调用方据此决定
/// 哪些库记录不能判定为已删除。
#[derive(Default)]
pub struct Walked {
    pub files: Vec<std::path::PathBuf>,
    pub warnings: Vec<String>,
    /// 本轮没有读取到的目录：打不开的（权限不足、网络盘掉线）和有意不跟随的链接目录。
    /// 这些子树里的文件一个都没看到，记录不能判定为已删除。
    pub skipped: Vec<std::path::PathBuf>,
}

pub fn scan_directory_recursive(dir: &Path) -> Result<Walked, String> {
    require_dir(dir)?;
    Ok(walk_filtered(dir, is_video_file, |path| fs::read_dir(path)))
}

pub fn scan_image_directory_recursive(dir: &Path) -> Result<Walked, String> {
    require_dir(dir)?;
    Ok(walk_filtered(dir, is_image_file, |path| fs::read_dir(path)))
}

/// 根目录本身打不开必须是错误而不是"扫到 0 个文件"——后者会被调用方当成
/// "全盘已删除"去清库
fn require_dir(dir: &Path) -> Result<(), String> {
    if dir.is_dir() {
        Ok(())
    } else {
        Err(format!("Not a directory: {}", dir.display()))
    }
}

/// 目录条目分成三类：文件、要进去的普通目录、不进去的链接目录。
enum Entry {
    File,
    Dir,
    Link,
}

/// Windows 上 `DirEntry::file_type` 把 junction/挂载点报成"既不是文件也不是目录"，
/// 所以这一类只能落到磁盘上实判一次；普通文件/普通目录走前两行，零额外开销。
fn classify(entry: &fs::DirEntry) -> Entry {
    match entry.file_type() {
        Ok(ft) if ft.is_file() => Entry::File,
        Ok(ft) if ft.is_dir() => {
            if ft.is_symlink() {
                Entry::Link
            } else {
                Entry::Dir
            }
        }
        _ => {
            let path = entry.path();
            if !path.is_dir() {
                return Entry::File;
            }
            if fs::read_link(&path).is_ok() {
                Entry::Link
            } else {
                Entry::Dir
            }
        }
    }
}

fn walk_filtered(
    root: &Path,
    wanted: impl Fn(&Path) -> bool,
    read_dir: impl Fn(&Path) -> Result<fs::ReadDir, std::io::Error>,
) -> Walked {
    let mut walk = Walked::default();
    let mut links: Vec<std::path::PathBuf> = Vec::new();
    // 显式栈而不是递归：栈深度由目录树决定，不会因为一条链接失控
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                // 单个目录读不了不影响其余部分：记一条告警继续走
                walk.skipped.push(dir.clone());
                walk.warnings.push(format!("无法读取目录 {}: {}", dir.display(), e));
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    walk.warnings.push(format!("{} 中有条目无法读取: {}", dir.display(), e));
                    continue;
                }
            };
            let path = entry.path();

            match classify(&entry) {
                Entry::File => {
                    if wanted(&path) {
                        walk.files.push(path);
                    }
                }
                Entry::Dir => {
                    // 回收站/系统目录里的 $R*.mp4 是已删除文件的副本，不能重新入库
                    let dir_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if dir_name != "$recycle.bin" && dir_name != "system volume information" {
                        stack.push(path);
                    }
                }
                // 链接目录（junction/目录软链/挂载点）不进：跟进去会把同一批文件按两条
                // 路径重复收录，一条指回祖先的链接还能让整盘扫描无限展开。
                // 根目录本身是链接时照常读取——那是用户自己选的入口。
                Entry::Link => links.push(path),
            }
        }
    }

    if !links.is_empty() {
        let named: Vec<String> = links
            .iter()
            .take(3)
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        walk.warnings.push(format!(
            "跳过 {} 个链接目录（不跟随 junction/软链）: {}",
            links.len(),
            named.join("、")
        ));
        walk.skipped.extend(links);
    }

    walk
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn added_at_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn file_size_of(path: &Path) -> i64 {
    fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0)
}

pub fn build_video(path: &std::path::PathBuf) -> Video {
    let metadata = get_metadata(&path.to_string_lossy()).ok();

    Video {
        id: uuid::Uuid::new_v4().to_string(),
        path: path.to_string_lossy().to_string(),
        filename: file_name(path),
        duration: metadata.as_ref().and_then(|m| m.duration),
        width: metadata.as_ref().and_then(|m| m.width),
        height: metadata.as_ref().and_then(|m| m.height),
        file_size: file_size_of(path),
        created_at: added_at_now(),
        thumbnail_path: None,
    }
}

/// 图片条目：ffprobe 读图片同样能给出宽高，取不到时留空由前端回退显示原图
pub fn build_image(path: &std::path::PathBuf) -> Image {
    let metadata = get_metadata(&path.to_string_lossy()).ok();
    let modified_at = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| {
            chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + d)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        });

    Image {
        id: uuid::Uuid::new_v4().to_string(),
        path: path.to_string_lossy().to_string(),
        filename: file_name(path),
        width: metadata.as_ref().and_then(|m| m.width),
        height: metadata.as_ref().and_then(|m| m.height),
        file_size: file_size_of(path),
        created_at: added_at_now(),
        thumbnail_path: None,
        modified_at,
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

/// 感知哈希（pHash）：ffmpeg 缩到 32×32 灰度 → 8×8 DCT-II 低频块 → 均值阈值化成 64 位。
/// 与字节级指纹互补：找"相似但不同"的连拍/截图系列；汉明距离 ≤ 阈值视为相似。
pub fn image_phash(path: &str) -> Option<u64> {
    const N: usize = 32;
    let output = hidden_command("ffmpeg")
        .args(image_input_opts(path))
        .args([
            "-i", path,
            "-vf", &format!("scale={}:{}:flags=bicubic,format=gray", N, N),
            "-f", "rawvideo",
            "-pix_fmt", "gray",
            "-",
        ])
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() < N * N {
        return None;
    }
    let mut pixels = [0f64; N * N];
    for (i, b) in output.stdout.iter().take(N * N).enumerate() {
        pixels[i] = *b as f64;
    }

    // 8×8 DCT-II：只取低频左上块
    const B: usize = 8;
    let mut dct = [[0f64; B]; B];
    for u in 0..B {
        let cu = if u == 0 { (1.0 / 2.0f64).sqrt() } else { 1.0 };
        for v in 0..B {
            let mut sum = 0.0;
            for x in 0..N {
                for y in 0..N {
                    let px = pixels[x * N + y];
                    let arg_x = std::f64::consts::PI * (2 * x + 1) as f64 * u as f64 / (2 * N) as f64;
                    let arg_y = std::f64::consts::PI * (2 * y + 1) as f64 * v as f64 / (2 * N) as f64;
                    sum += px * arg_x.cos() * arg_y.cos();
                }
            }
            dct[v][u] = 0.5 * cu * sum;
        }
    }

    // 均值阈值化（跳过 DC 分量 dct[0][0]，它只反映整体亮度）
    let mut total = 0.0;
    for v in 0..B {
        for u in 0..B {
            if !(u == 0 && v == 0) {
                total += dct[v][u];
            }
        }
    }
    let mean = total / ((B * B - 1) as f64);
    let mut hash: u64 = 0;
    for v in 0..B {
        for u in 0..B {
            if u == 0 && v == 0 {
                continue;
            }
            hash = (hash << 1) | if dct[v][u] > mean { 1 } else { 0 };
        }
    }
    Some(hash)
}

/// 64 位哈希的汉明距离
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
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
/// 传入 duration 为 None 时对图片同样适用（seek 0 + 单帧 = 缩放导出）。
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
            .args(image_input_opts(video_path))
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
        // 视频扫描不能把图片收进来
        assert!(!is_video_file(&PathBuf::from("photo.jpg")));
    }

    #[test]
    fn test_is_image_file() {
        assert!(is_image_file(&PathBuf::from("photo.JPG")));
        assert!(is_image_file(&PathBuf::from("icon.png")));
        assert!(is_image_file(&PathBuf::from("pic.webp")));
        assert!(!is_image_file(&PathBuf::from("clip.mp4")));
        assert!(!is_image_file(&PathBuf::from("notes.txt")));
        // 系统装了 HEVC 也解不了 HEIC，浏览器只会显示裂图，故不入库
        assert!(!is_image_file(&PathBuf::from("camera.heic")));
    }

    #[test]
    fn test_scan_separates_images_from_videos() {
        let dir = std::env::temp_dir().join(format!("viewman_kind_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.mp4"), b"x").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("b.png"), b"x").unwrap();

        let videos = scan_directory_recursive(&dir).unwrap();
        let images = scan_image_directory_recursive(&dir).unwrap();
        assert_eq!(videos.files.len(), 1);
        assert_eq!(images.files.len(), 1);
        assert!(images.files[0].ends_with("b.png"));

        fs::remove_dir_all(&dir).unwrap();
    }

    /// 单个目录读不了（权限不足、网络盘掉线）不能中断整盘扫描：其余文件照常收，
    /// 同时把该目录记进 skipped，供调用方跳过"已删除"判定
    #[test]
    fn test_walk_skips_unreadable_dir_and_continues() {
        let dir = std::env::temp_dir().join(format!("viewman_locked_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("locked").join("deep")).unwrap();
        fs::write(dir.join("a.mp4"), b"x").unwrap();
        fs::write(dir.join("locked").join("deep").join("b.mp4"), b"x").unwrap();

        let blocked = dir.join("locked");
        let walked = walk_filtered(&dir, is_video_file, |path| {
            if path == blocked.as_path() {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            } else {
                fs::read_dir(path)
            }
        });

        assert_eq!(walked.files.len(), 1, "被跳过的子树不该混进结果，实际: {:?}", walked.files);
        assert_eq!(walked.skipped, vec![blocked]);
        assert_eq!(walked.warnings.len(), 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    /// 回归：库里放一条指回自身的 junction，扫描必须走完而不是无限递归；
    /// 链接目录整个不跟随，但记进 skipped 以免其下记录被判定为已删除
    #[cfg(windows)]
    #[test]
    fn test_scan_survives_self_referencing_junction() {
        let dir = std::env::temp_dir().join(format!("viewman_cycle_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.mp4"), b"x").unwrap();
        fs::write(dir.join("sub").join("b.mp4"), b"x").unwrap();

        let link = dir.join("loop");
        let made = Command::new("cmd")
            .args(["/C", "mklink", "/J", &link.to_string_lossy(), &dir.to_string_lossy()])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !made {
            eprintln!("跳过：本机无法创建 junction");
            fs::remove_dir(&link).ok();
            fs::remove_dir_all(&dir).ok();
            return;
        }

        let walked = scan_directory_recursive(&dir).unwrap();
        assert_eq!(walked.files.len(), 2, "每个真实文件只应被看到一次，实际: {:?}", walked.files);
        assert_eq!(walked.skipped, vec![link.clone()]);
        assert_eq!(walked.warnings.len(), 1);

        fs::remove_dir(&link).ok();
        fs::remove_dir_all(&dir).ok();
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

    /// 图片封面复用同一条 ffmpeg 通路：seek 0 + 单帧 = 缩放导出
    #[test]
    fn test_extract_thumbnail_from_image_file() {
        if !ffmpeg_available() {
            eprintln!("跳过：本机未安装 ffmpeg");
            return;
        }
        let dir = std::env::temp_dir().join(format!("viewman_imgthumb_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let png = dir.join("source.png");
        let generated = Command::new("ffmpeg")
            .args([
                "-y",
                "-f", "lavfi",
                "-i", "testsrc=size=1200x800:rate=1:duration=1",
                "-frames:v", "1",
                &png.to_string_lossy(),
            ])
            .output()
            .unwrap();
        assert!(generated.status.success(), "生成测试图片失败");

        let out = dir.join("thumb.jpg");
        extract_thumbnail(&png.to_string_lossy(), &out, None).unwrap();
        assert!(fs::metadata(&out).unwrap().len() > 0, "图片封面应为非空文件");

        let probe = get_metadata(&png.to_string_lossy()).unwrap();
        assert_eq!(probe.width, Some(1200));
        assert_eq!(probe.height, Some(800));

        fs::remove_dir_all(&dir).unwrap();
    }

    /// 回归：文件名里的 `%` 会被 image2 当成序号模板，图片的尺寸/封面/哈希全部读不到
    /// （库里 8820 张 QQ 转存图就是这么丢尺寸的），同时确认视频不受该选项影响
    #[test]
    fn test_percent_in_filename_reads_image_and_video() {
        if !ffmpeg_available() {
            eprintln!("跳过：本机未安装 ffmpeg");
            return;
        }
        let dir = std::env::temp_dir().join(format!("viewman_pct_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let png = dir.join("$$%BL9DM1T{PSA~77JD@YN7.png");
        assert!(Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", "testsrc=size=640x480:rate=1:duration=1", "-frames:v", "1", &png.to_string_lossy()])
            .output().unwrap().status.success(), "生成测试图片失败");

        let meta = get_metadata(&png.to_string_lossy()).unwrap();
        assert_eq!((meta.width, meta.height), (Some(640), Some(480)), "带 % 的图片应读到尺寸");
        let thumb = dir.join("pct-thumb.jpg");
        extract_thumbnail(&png.to_string_lossy(), &thumb, None).unwrap();
        assert!(fs::metadata(&thumb).unwrap().len() > 0, "带 % 的图片应能出封面");
        assert!(image_phash(&png.to_string_lossy()).is_some(), "带 % 的图片应能算 pHash");

        // 视频名里带 % 也照常工作（pattern_type 是 image2 专有选项，不能无条件塞给 ffmpeg）
        let video = dir.join("sample 100%.mp4");
        assert!(Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=2", "-pix_fmt", "yuv420p", &video.to_string_lossy()])
            .output().unwrap().status.success(), "生成测试视频失败");
        let vmeta = get_metadata(&video.to_string_lossy()).unwrap();
        assert_eq!((vmeta.width, vmeta.height), (Some(320), Some(240)));
        assert!(vmeta.duration.unwrap_or(0.0) > 1.0, "应读到视频时长");
        let vthumb = dir.join("pct-video-thumb.jpg");
        extract_thumbnail(&video.to_string_lossy(), &vthumb, Some(2.0)).unwrap();
        assert!(fs::metadata(&vthumb).unwrap().len() > 0);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_scan_empty_directory() {        let dir = std::env::temp_dir().join(format!("viewman_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let result = scan_directory_recursive(&dir);
        assert!(result.is_ok());
        assert!(result.unwrap().files.is_empty());

        fs::remove_dir(&dir).unwrap();
    }

    /// 根目录不存在必须是 Err，不能退化成"扫到 0 个文件"——调用方会把 0 当作"全盘已删除"
    #[test]
    fn test_scan_missing_root_is_error() {
        let dir = std::env::temp_dir().join(format!("viewman_absent_{}", uuid::Uuid::new_v4()));
        assert!(scan_directory_recursive(&dir).is_err());
        assert!(scan_image_directory_recursive(&dir).is_err());
    }
}
