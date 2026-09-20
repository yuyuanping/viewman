use std::io::Read;
use std::path::{Path, PathBuf};

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::models::{ConversionResult, Video, VideoFileStatus};
use crate::scanner;

use super::AppState;

/// 按文件头魔数识别真实图片类型，返回 (图片扩展名, 格式名)
fn sniff_image_format(header: &[u8]) -> Option<(&'static str, &'static str)> {
    match header {
        [0xFF, 0xD8, 0xFF, ..] => Some(("jpg", "JPEG")),
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some(("png", "PNG")),
        [b'G', b'I', b'F', b'8', ..] => Some(("gif", "GIF")),
        [b'B', b'M', ..] => Some(("bmp", "BMP")),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some(("webp", "WebP")),
        _ => None,
    }
}

fn read_header(path: &Path) -> Option<[u8; 16]> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 16];
    let n = file.read(&mut header).ok()?;
    Some(header[..n].try_into().unwrap_or(header))
}

/// 为重名后的文件寻找不冲突的路径：`name.jpg`、`name (2).jpg`、`name (3).jpg` …
fn unique_path(target: &Path) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let ext = target.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
    for i in 2..1000 {
        let candidate = target.with_file_name(format!("{stem} ({i}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    target.to_path_buf()
}

#[tauri::command]
pub fn get_videos(state: State<AppState>) -> Result<Vec<Video>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_all_videos(&conn).map_err(|e| e.to_string())
}

/// 打开文件并读取文件头：确认可读性，同时用魔数识别"图片伪装成视频"的假视频
#[tauri::command]
pub fn check_video_file(state: State<AppState>, video_id: String) -> Result<VideoFileStatus, String> {
    use std::io::ErrorKind;
    let path = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_video_path(&conn, &video_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Video not found: {}", video_id))?
    };

    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Ok(VideoFileStatus {
            status: "missing".into(),
            message: Some("文件不存在（磁盘可能未连接，或文件已被移动/删除）".into()),
        });
    }
    if p.is_dir() {
        return Ok(VideoFileStatus {
            status: "missing".into(),
            message: Some("该路径指向文件夹而不是视频文件".into()),
        });
    }
    match std::fs::File::open(&p) {
        Ok(mut file) => {
            let mut header = [0u8; 16];
            let n = file.read(&mut header).unwrap_or(0);
            if let Some((_, label)) = sniff_image_format(&header[..n]) {
                return Ok(VideoFileStatus {
                    status: "fake_image".into(),
                    message: Some(format!("文件内容实为 {label} 图片，并非视频")),
                });
            }
            Ok(VideoFileStatus { status: "readable".into(), message: None })
        }
        Err(e) if e.kind() == ErrorKind::PermissionDenied => Ok(VideoFileStatus {
            status: "unavailable".into(),
            message: Some("没有读取该文件的权限".into()),
        }),
        Err(e) => Ok(VideoFileStatus {
            status: "unavailable".into(),
            message: Some(format!("文件被占用或无法打开: {}", e)),
        }),
    }
}

#[tauri::command]
pub fn delete_video(
    app: tauri::AppHandle,
    state: State<AppState>,
    video_id: String,
) -> Result<(), String> {
    let path = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_video_path(&conn, &video_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Video not found: {}", video_id))?
    };

    trash::delete(&path).map_err(|e| format!("删除文件失败: {}", e))?;

    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::delete_video(&conn, &video_id).map_err(|e| e.to_string())?;
    }

    clear_thumbnail_cache(&app, &video_id);
    Ok(())
}

/// 封面缓存文件名由视频 id 决定，视频移除后一并清理，避免留下孤儿文件
fn clear_thumbnail_cache(app: &tauri::AppHandle, video_id: &str) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join("thumbnails").join(format!("{}.jpg", video_id)));
    }
}

/// 把"内容实为图片"的假视频转换成图片：按真实格式另存为 .jpg/.png 等新文件，
/// 源文件移入回收站，并从视频库删除记录
#[tauri::command]
pub fn convert_fake_images(
    app: tauri::AppHandle,
    state: State<AppState>,
    video_ids: Vec<String>,
) -> Result<ConversionResult, String> {
    let mut result = ConversionResult::default();
    for video_id in video_ids {
        let path = {
            let conn = state.db.lock().map_err(|e| e.to_string())?;
            db::get_video_path(&conn, &video_id)
                .map_err(|e| e.to_string())?
        };
        let Some(path) = path else {
            result.errors.push(format!("记录不存在，已跳过: {}", video_id));
            continue;
        };

        let p = Path::new(&path);
        let Some((ext, _label)) = read_header(p).as_ref().and_then(|h| sniff_image_format(h)) else {
            result.errors.push(format!("{} 当前内容不是图片，已跳过", p.display()));
            continue;
        };

        let target = unique_path(&p.with_extension(ext));
        if let Err(e) = std::fs::copy(p, &target) {
            result.errors.push(format!("复制 {} 失败: {}", p.display(), e));
            continue;
        }
        if let Err(e) = trash::delete(p) {
            let _ = std::fs::remove_file(&target);
            result.errors.push(format!("源文件移入回收站失败，已保留 {}: {}", p.display(), e));
            continue;
        }

        let removal = state
            .db
            .lock()
            .map_err(|e| e.to_string())
            .and_then(|conn| db::delete_video(&conn, &video_id).map_err(|e| e.to_string()));
        if let Err(e) = removal {
            // 图片和回收站都已处理，仅删库失败：还原源文件，保持库记录与磁盘一致
            let _ = std::fs::copy(&target, p);
            let _ = std::fs::remove_file(&target);
            result.errors.push(format!("删除库记录失败，已还原源文件: {}", e));
            continue;
        }

        clear_thumbnail_cache(&app, &video_id);
        result.converted += 1;
    }
    Ok(result)
}

/// 短视频转图片的判定阈值：时长 ≤5 秒且去重画面 ≤3 帧（1 秒内直接视为静图）
const SHORT_IMAGE_MAX_SECONDS: f64 = 5.0;
const SHORT_IMAGE_MAX_UNIQUE_FRAMES: usize = 3;

/// 扫描出"近似静图"的超短视频 id：时长 ≤5 秒，且 <1 秒或 mpdecimate 去重后画面 ≤3 帧
#[tauri::command]
pub async fn find_static_videos(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    use crate::scanner;

    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法检测短视频。请安装 ffmpeg 并加入 PATH。".into());
    }

    let jobs: Vec<(String, String, f64)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_videos_with_max_duration(&conn, SHORT_IMAGE_MAX_SECONDS).map_err(|e| e.to_string())?
    };

    let ids = tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for (id, path, duration) in jobs {
            if !Path::new(&path).exists() {
                continue;
            }
            if duration < 1.0 {
                out.push(id);
                continue;
            }
            if matches!(scanner::unique_frame_count(&path), Some(n) if n <= SHORT_IMAGE_MAX_UNIQUE_FRAMES) {
                out.push(id);
            }
        }
        out
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(ids)
}

/// 把近似静图的超短视频（≤5 秒）转换成图片：ffmpeg 抽首帧生成同名 .jpg，
/// 原视频移入回收站，并从视频库删除记录。≥1 秒的候选会先复核画面是否近似静图。
#[tauri::command]
pub async fn convert_short_videos(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    video_ids: Vec<String>,
) -> Result<ConversionResult, String> {
    use crate::scanner;

    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法抽帧转换。请安装 ffmpeg 并加入 PATH。".into());
    }

    let jobs: Vec<(String, String, Option<f64>)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for id in video_ids {
            match db::get_video_path_and_duration(&conn, &id) {
                Ok(Some((path, duration))) => out.push((id, path, duration)),
                Ok(None) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        out
    };

    let (converted_ids, errors) = tauri::async_runtime::spawn_blocking(move || {
        let mut converted: Vec<String> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for (id, path, duration) in jobs {
            let p = Path::new(&path);
            if !p.exists() {
                errors.push(format!("文件不存在，已跳过: {}", path));
                continue;
            }
            let need_still_check = !matches!(duration, Some(d) if d < 1.0);
            if need_still_check {
                match scanner::unique_frame_count(&path) {
                    Some(n) if n <= SHORT_IMAGE_MAX_UNIQUE_FRAMES => {}
                    Some(_) => {
                        errors.push(format!("{} 画面有动态变化，已跳过", p.display()));
                        continue;
                    }
                    None => {
                        errors.push(format!("{} 无法解码，已跳过", p.display()));
                        continue;
                    }
                }
            }
            let target = unique_path(&p.with_extension("jpg"));
            if let Err(e) = scanner::extract_full_frame(&path, &target) {
                let _ = std::fs::remove_file(&target);
                errors.push(format!("{} 抽帧失败: {}", p.display(), e));
                continue;
            }
            if let Err(e) = trash::delete(&path) {
                let _ = std::fs::remove_file(&target);
                errors.push(format!("移入回收站失败，已保留 {}: {}", p.display(), e));
                continue;
            }
            converted.push(id);
        }
        (converted, errors)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut result = ConversionResult { converted: 0, errors };
    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        for id in &converted_ids {
            db::delete_video(&conn, id).map_err(|e| e.to_string())?;
            result.converted += 1;
        }
    }
    for id in &converted_ids {
        clear_thumbnail_cache(&app, id);
    }
    Ok(result)
}

/// 文件移动：同盘 rename；跨盘（Windows ERROR_NOT_SAME_DEVICE）回退为复制+删源
fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(17) => {
            std::fs::copy(src, dst).map_err(|e| format!("跨盘复制失败: {}", e))?;
            std::fs::remove_file(src)
                .map_err(|e| format!("复制成功但删除源文件失败，请手动清理: {}", e))?;
            Ok(())
        }
        Err(e) => Err(format!("移动失败: {}", e)),
    }
}

/// 把视频文件移动到目标文件夹并同步库记录路径；目标重名时自动加 " (2)" 后缀，
/// 写库失败会把文件移回原位
#[tauri::command]
pub async fn move_video(
    state: State<'_, AppState>,
    video_id: String,
    target_dir: String,
) -> Result<String, String> {
    let (old_path, new_path) = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let old_path = db::get_video_path(&conn, &video_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Video not found: {}", video_id))?;
        let src = Path::new(&old_path);
        let filename = src
            .file_name()
            .ok_or_else(|| "源文件路径无效".to_string())?
            .to_string_lossy()
            .to_string();
        let dir = Path::new(&target_dir);
        if dir
            .join(&filename)
            .to_string_lossy()
            .eq_ignore_ascii_case(&old_path)
        {
            return Err("文件已经在该目录中".into());
        }
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
        let ext = src.extension().and_then(|s| s.to_str());
        let mut candidate = dir.join(&filename);
        let mut idx = 1;
        while candidate.exists()
            || db::is_path_taken(&conn, &candidate.to_string_lossy(), &video_id)
                .map_err(|e| e.to_string())?
        {
            idx += 1;
            if idx > 999 {
                return Err("目标目录同名文件过多".into());
            }
            let name = match ext {
                Some(e) => format!("{stem} ({idx}).{e}"),
                None => format!("{stem} ({idx})"),
            };
            candidate = dir.join(name);
        }
        (old_path, candidate.to_string_lossy().to_string())
    };

    let (src, dst) = (old_path.clone(), new_path.clone());
    let move_result = tauri::async_runtime::spawn_blocking(move || {
        move_file(Path::new(&src), Path::new(&dst))
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Err(e) = move_result {
        return Err(e);
    }

    if let Err(e) = state
        .db
        .lock()
        .map_err(|e| e.to_string())
        .and_then(|conn| db::update_video_path(&conn, &video_id, &new_path).map_err(|e| e.to_string()))
    {
        let _ = move_file(Path::new(&new_path), Path::new(&old_path));
        return Err(format!("更新库记录失败，已还原文件位置: {}", e));
    }
    Ok(new_path)
}

#[derive(Clone, serde::Serialize)]
pub struct HevcProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    pub converted: usize,
    pub failed: usize,
}

#[derive(Clone, serde::Serialize)]
pub struct HevcDetectProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
}

/// 检测库中的 HEVC 视频 id（ffprobe 读视频流编码名），文件不存在的跳过。
/// 逐个检测经 hevc-detect-progress 事件推送进度。
#[tauri::command]
pub async fn find_hevc_videos(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg/ffprobe，无法识别编码。请安装 ffmpeg 并加入 PATH。".into());
    }
    let jobs: Vec<(String, String)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_all_videos(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|v| (v.id, v.path))
            .filter(|(_, p)| Path::new(p).exists())
            .collect()
    };
    let total = jobs.len();
    let task_app = app.clone();
    let ids = tauri::async_runtime::spawn_blocking(move || {
        let mut found = Vec::new();
        for (index, (id, path)) in jobs.into_iter().enumerate() {
            if scanner::video_codec_name(&path).as_deref() == Some("hevc") {
                found.push(id);
            }
            let _ = task_app.emit(
                "hevc-detect-progress",
                HevcDetectProgress { processed: index + 1, total, done: false },
            );
        }
        let _ = task_app.emit(
            "hevc-detect-progress",
            HevcDetectProgress { processed: total, total, done: true },
        );
        found
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(ids)
}

/// HEVC 永久转码：就地重编码为 H.264 + AAC。流程为 转码到同目录临时文件 →
/// 原文件移入回收站 → 临时文件改回原名（库路径保持稳定），并清掉按需转码缓存。
/// 单个失败不影响其余；进度经 hevc-progress 事件推送。
#[tauri::command]
pub async fn convert_hevc_videos(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    video_ids: Vec<String>,
) -> Result<ConversionResult, String> {
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法转码。请安装 ffmpeg 并加入 PATH。".into());
    }
    let jobs: Vec<(String, String)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for id in video_ids {
            if let Ok(Some(path)) = db::get_video_path(&conn, &id) {
                out.push((id, path));
            }
        }
        out
    };
    let transcoded_dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("transcoded");
    let total = jobs.len();
    let task_app = app.clone();

    let (done, errors) = tauri::async_runtime::spawn_blocking(move || {
        let mut done: Vec<(String, String, i64)> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        let mut converted = 0usize;
        for (index, (id, path)) in jobs.iter().enumerate() {
            let p = Path::new(path);
            let emit = |converted: usize, failed: usize| {
                let _ = task_app.emit(
                    "hevc-progress",
                    HevcProgress {
                        processed: index + 1,
                        total,
                        done: false,
                        converted,
                        failed,
                    },
                );
            };
            if !p.exists() {
                errors.push(format!("文件不存在，已跳过: {}", path));
                emit(converted, errors.len());
                continue;
            }
            if scanner::video_codec_name(path).as_deref() != Some("hevc") {
                emit(converted, errors.len());
                continue;
            }
            let tmp = p.with_file_name(format!(".viewman-h264-{}.tmp.mp4", id));
            if let Err(e) = scanner::transcode_to_h264(path, &tmp) {
                let _ = std::fs::remove_file(&tmp);
                errors.push(format!("{} 转码失败: {}", p.display(), e));
                emit(converted, errors.len());
                continue;
            }
            if let Err(e) = trash::delete(path) {
                let _ = std::fs::remove_file(&tmp);
                errors.push(format!("{} 移入回收站失败（可能被占用），已中止: {}", p.display(), e));
                emit(converted, errors.len());
                continue;
            }
            // 极少数：回收站未让位。回退到不覆盖的重命名，路径变化需写库
            let target = if p.exists() { unique_path(p) } else { p.to_path_buf() };
            if let Err(e) = std::fs::rename(&tmp, &target) {
                let _ = std::fs::remove_file(&tmp);
                errors.push(format!(
                    "{} 已进回收站但转码文件写回失败（{}），请从回收站还原",
                    p.display(), e
                ));
                emit(converted, errors.len());
                continue;
            }
            let _ = std::fs::remove_file(transcoded_dir.join(format!("{}.mp4", id)));
            let size = std::fs::metadata(&target).map(|m| m.len() as i64).unwrap_or(0);
            done.push((id.clone(), target.to_string_lossy().to_string(), size));
            converted += 1;
            emit(converted, errors.len());
        }
        (done, errors)
    })
    .await
    .map_err(|e| e.to_string())?;

    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        for (id, path, size) in &done {
            db::update_video_location(&conn, id, path, *size).map_err(|e| e.to_string())?;
        }
    }
    let _ = app.emit(
        "hevc-progress",
        HevcProgress { processed: total, total, done: true, converted: done.len(), failed: errors.len() },
    );
    Ok(ConversionResult { converted: done.len(), errors })
}

/// 重复检测内容指纹：文件大小 + 首/中/尾各 256KB 采样哈希（SipHash）。
/// 视频内容相同则指纹相同；不同文件冲撞概率可忽略，且指纹相同前已按大小分组。
fn duplicate_signature(path: &str, size: i64) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    use std::io::{Read, Seek, SeekFrom};

    const SAMPLE: u64 = 256 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    size.hash(&mut hasher);
    let span = size.max(0) as u64;
    let mut digest_at = |offset: u64| -> Option<()> {
        let len = std::cmp::min(SAMPLE, span.saturating_sub(offset)) as usize;
        if len == 0 {
            return Some(());
        }
        let mut buf = vec![0u8; len];
        file.seek(SeekFrom::Start(offset)).ok()?;
        file.read_exact(&mut buf).ok()?;
        buf.hash(&mut hasher);
        Some(())
    };
    digest_at(0)?;
    digest_at(span / 2)?;
    digest_at(span.saturating_sub(SAMPLE))?;
    Some(hasher.finish())
}

/// 找出内容相同的重复视频组：先按文件大小分组，再用内容指纹细分。
/// 每组按添加时间升序返回（第一个视为要保留的原件）
#[tauri::command]
pub async fn find_duplicate_videos(state: State<'_, AppState>) -> Result<Vec<Vec<String>>, String> {
    use std::collections::HashMap;

    let jobs: Vec<(String, String, i64, String)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_all_videos(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|v| (v.id, v.path, v.file_size, v.created_at))
            .filter(|(_, p, _, _)| Path::new(p).exists())
            .collect()
    };

    let groups = tauri::async_runtime::spawn_blocking(move || {
        let mut by_size: HashMap<i64, Vec<(String, String, String)>> = HashMap::new();
        for (id, path, size, created_at) in jobs {
            by_size.entry(size).or_default().push((id, path, created_at));
        }
        let mut out: Vec<Vec<(String, String)>> = Vec::new();
        for (size, entries) in by_size {
            if entries.len() < 2 {
                continue;
            }
            let mut by_sig: HashMap<u64, Vec<(String, String)>> = HashMap::new();
            for (id, path, created_at) in entries {
                if let Some(sig) = duplicate_signature(&path, size) {
                    by_sig.entry(sig).or_default().push((id, created_at));
                }
            }
            for group in by_sig.into_values().filter(|g| g.len() > 1) {
                out.push(group);
            }
        }
        out.sort_by_key(|g| std::cmp::Reverse(g.len()));
        out.into_iter()
            .map(|mut g| {
                g.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                g.into_iter().map(|(id, _)| id).collect()
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(groups)
}

/// 给内置播放器提供可播放路径：WebView2 没有 HEVC 解码器，遇到 hevc 源
/// 按需转码成 H.264 缓存到 `<app_data>/transcoded/<id>.mp4`；其他编码直接返回原路径。
/// 转码失败也返回原路径，让播放器走既有的错误提示流程。
#[tauri::command]
pub async fn get_playable_path(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    video_id: String,
) -> Result<String, String> {
    let path = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_video_path(&conn, &video_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Video not found: {}", video_id))?
    };

    let probe_path = path.clone();
    let codec = tauri::async_runtime::spawn_blocking(move || scanner::video_codec_name(&probe_path))
        .await
        .map_err(|e| e.to_string())?;
    if codec.as_deref() != Some("hevc") {
        return Ok(path);
    }

    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("transcoded");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{}.mp4", video_id));
    if out.exists() {
        let fresh = std::fs::metadata(&path)
            .and_then(|src| Ok(std::fs::metadata(&out)?.modified()? >= src.modified()?))
            .unwrap_or(false);
        if fresh {
            return Ok(out.to_string_lossy().to_string());
        }
    }

    let src = path.clone();
    let out_str = out.to_string_lossy().to_string();
    let job = tauri::async_runtime::spawn_blocking(move || scanner::transcode_to_h264(&src, &out))
        .await
        .map_err(|e| e.to_string())?;
    if job.is_ok() {
        Ok(out_str)
    } else {
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sniff_image_format() {
        assert_eq!(sniff_image_format(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some(("jpg", "JPEG")));
        assert_eq!(
            sniff_image_format(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
            Some(("png", "PNG"))
        );
        assert_eq!(sniff_image_format(b"GIF89a"), Some(("gif", "GIF")));
        assert_eq!(sniff_image_format(b"BM...."), Some(("bmp", "BMP")));
        assert_eq!(
            sniff_image_format(b"RIFF\x04\x00\x00\x00WEBPVP8 "),
            Some(("webp", "WebP"))
        );
        // RIFF 但非 WEBP（如 wav/avi）不算图片
        assert_eq!(sniff_image_format(b"RIFF\x04\x00\x00\x00WAVE"), None);
        // 正常 mp4（ftyp box）
        assert_eq!(sniff_image_format(b"\x00\x00\x00\x18ftypmp42"), None);
        assert_eq!(sniff_image_format(&[]), None);
    }

    #[test]
    fn test_duplicate_signature_detects_same_and_diff() {
        let dir = std::env::temp_dir().join(format!("viewman-dup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 600KB：超出首尾采样范围，中段差异必须能被识别
        let make = |name: &str, mid_byte: u8| {
            let p = dir.join(name);
            let mut data = vec![0xA5u8; 600 * 1024];
            data[300 * 1024] = mid_byte;
            std::fs::write(&p, &data).unwrap();
            (p.to_string_lossy().to_string(), data.len() as i64)
        };
        let (a, size) = make("a.bin", 1);
        let (b, _) = make("b.bin", 1);
        let (c, _) = make("c.bin", 2);
        assert_eq!(
            duplicate_signature(&a, size),
            duplicate_signature(&b, size),
            "内容相同的文件指纹应一致"
        );
        assert_ne!(
            duplicate_signature(&a, size),
            duplicate_signature(&c, size),
            "仅中段不同的同大小文件指纹应不同"
        );
        assert_eq!(duplicate_signature(&dir.join("nope.bin").to_string_lossy(), 10), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_unique_path_avoids_collision() {
        let dir = std::env::temp_dir().join(format!("viewman-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let taken = dir.join("a.jpg");
        std::fs::write(&taken, b"x").unwrap();

        assert_eq!(unique_path(&dir.join("b.jpg")), dir.join("b.jpg"));
        assert_eq!(unique_path(&taken), dir.join("a (2).jpg"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
