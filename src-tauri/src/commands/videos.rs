use std::io::Read;
use std::path::{Path, PathBuf};

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::models::{ConversionResult, Image, Video, VideoFileStatus};
use crate::scanner;

use super::thumbnails::clear_thumbnail_cache;
use super::{flush, flush_with_retry, read_cache, undeleted_targets, write_cache, AppState, MapErrStr, VIDEO_DUPLICATE_CACHE};

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
    let conn = state.db.lock().map_err_str()?;
    db::get_all_videos(&conn).map_err_str()
}

/// 打开文件并读取文件头：确认可读性，同时用魔数识别"图片伪装成视频"的假视频
#[tauri::command]
pub fn check_video_file(state: State<AppState>, video_id: String) -> Result<VideoFileStatus, String> {
    use std::io::ErrorKind;
    let path = {
        let conn = state.db.lock().map_err_str()?;
        db::get_video_path(&conn, &video_id)
            .map_err_str()?
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

/// 批量删除视频：一次回收站事务 + 一次数据库事务，返回成功删除的 id。
/// 与图片库同一条通路，勾选几十个不再逐个过桥。
#[tauri::command]
pub async fn delete_videos(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    video_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    let targets: Vec<(String, String)> = {
        let conn = state.db.lock().map_err_str()?;
        let mut out = Vec::with_capacity(video_ids.len());
        for video_id in &video_ids {
            let path = db::get_video_path(&conn, video_id)
                .map_err_str()?
                .ok_or_else(|| format!("Video not found: {}", video_id))?;
            out.push((video_id.clone(), path));
        }
        out
    };

    let for_files = targets.clone();
    let survivors = tauri::async_runtime::spawn_blocking(move || undeleted_targets(&for_files))
        .await
        .map_err_str()?;
    let failed: std::collections::HashSet<String> = survivors.into_iter().map(|(id, _)| id).collect();
    let deleted: Vec<String> = targets
        .iter()
        .filter(|(id, _)| !failed.contains(id))
        .map(|(id, _)| id.clone())
        .collect();

    if !deleted.is_empty() {
        let conn = state.db.lock().map_err_str()?;
        let tx = conn.unchecked_transaction().map_err_str()?;
        db::delete_videos_by_ids(&tx, &deleted).map_err_str()?;
        tx.commit().map_err_str()?;
        for id in &deleted {
            clear_thumbnail_cache(&app, id);
        }
    }

    Ok(deleted)
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
            let conn = state.db.lock().map_err_str()?;
            db::get_video_path(&conn, &video_id)
                .map_err_str()?
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
            .map_err_str()
            .and_then(|conn| db::delete_video(&conn, &video_id).map_err_str());
        if let Err(e) = removal {
            // 图片和回收站都已处理，仅删库失败：还原源文件，保持库记录与磁盘一致
            let _ = std::fs::copy(&target, p);
            let _ = std::fs::remove_file(&target);
            result.errors.push(format!("删除库记录失败，已还原源文件: {}", e));
            continue;
        }

        clear_thumbnail_cache(&app, &video_id);
        // 转换结果顺手登记进图片库，否则它只是一张磁盘上无人索引的孤儿图
        let image = scanner::build_image(&target);
        if let Ok(conn) = state.db.lock() {
            let _ = db::insert_image(&conn, &image);
        }
        result.converted += 1;
    }
    Ok(result)
}

/// 短视频转图片的判定阈值：时长 ≤5 秒且去重画面 ≤3 帧（1 秒内直接视为静图）
const SHORT_IMAGE_MAX_SECONDS: f64 = 5.0;
const SHORT_IMAGE_MAX_UNIQUE_FRAMES: usize = 3;

/// 扫描可转图片的条目：时长 ≤5 秒且 <1 秒或 mpdecimate 去重后 ≤3 帧的静图视频，
/// 以及时长取不到但魔数是图片的伪装文件
#[tauri::command]
pub async fn find_static_videos(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    use crate::scanner;

    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法检测短视频。请安装 ffmpeg 并加入 PATH。".into());
    }

    let jobs: Vec<(String, String, Option<f64>)> = {
        let conn = state.db.lock().map_err_str()?;
        db::get_videos_with_max_duration(&conn, SHORT_IMAGE_MAX_SECONDS).map_err_str()?
    };

    let ids = tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for (id, path, duration) in jobs {
            let p = Path::new(&path);
            if !p.exists() {
                continue;
            }
            match duration {
                None => {
                    // 无时长：可能是图片伪装成视频，魔数命中即可转图片
                    let is_image = read_header(p).and_then(|h| sniff_image_format(&h)).is_some();
                    if is_image {
                        out.push(id);
                    }
                }
                Some(d) if d < 1.0 => out.push(id),
                Some(_) => {
                    if matches!(scanner::unique_frame_count(&path), Some(n) if n <= SHORT_IMAGE_MAX_UNIQUE_FRAMES) {
                        out.push(id);
                    }
                }
            }
        }
        out
    })
    .await
    .map_err_str()?;

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
        let conn = state.db.lock().map_err_str()?;
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

    let (converted, errors) = tauri::async_runtime::spawn_blocking(move || {
        let mut converted: Vec<(String, Image)> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for (id, path, duration) in jobs {
            let p = Path::new(&path);
            if !p.exists() {
                errors.push(format!("文件不存在，已跳过: {}", path));
                continue;
            }
            // 图片伪装成视频：魔数命中则直接按真实格式另存，无需 ffmpeg
            if let Some((ext, _)) = read_header(p).and_then(|h| sniff_image_format(&h)) {
                let target = unique_path(&p.with_extension(ext));
                if let Err(e) = std::fs::copy(p, &target) {
                    errors.push(format!("复制 {} 失败: {}", p.display(), e));
                    continue;
                }
                if let Err(e) = trash::delete(p) {
                    let _ = std::fs::remove_file(&target);
                    errors.push(format!("源文件移入回收站失败，已保留 {}: {}", p.display(), e));
                    continue;
                }
                converted.push((id, scanner::build_image(&target)));
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
            converted.push((id, scanner::build_image(&target)));
        }
        (converted, errors)
    })
    .await
    .map_err_str()?;

    let mut result = ConversionResult { converted: 0, errors };
    {
        let conn = state.db.lock().map_err_str()?;
        for (id, image) in &converted {
            db::delete_video(&conn, id).map_err_str()?;
            // 转换结果顺手登记进图片库，否则它只是一张磁盘上无人索引的孤儿图
            let _ = db::insert_image(&conn, image);
            result.converted += 1;
        }
    }
    for (id, _) in &converted {
        clear_thumbnail_cache(&app, id);
    }
    Ok(result)
}

/// 文件移动：同盘 rename；跨盘（Windows ERROR_NOT_SAME_DEVICE）回退为复制+删源
pub(crate) fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
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
        let conn = state.db.lock().map_err_str()?;
        let old_path = db::get_video_path(&conn, &video_id)
            .map_err_str()?
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
                .map_err_str()?
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
    .map_err_str()?;
    if let Err(e) = move_result {
        return Err(e);
    }

    if let Err(e) = state
        .db
        .lock()
        .map_err_str()
        .and_then(|conn| db::update_video_path(&conn, &video_id, &new_path).map_err_str())
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

/// 检测库中的 HEVC 视频 id：ffprobe 逐文件探测编码并写入 video_codec 列，
/// 结果持久化——重启后只需探测新文件。进度经 hevc-detect-progress 事件推送。
#[tauri::command]
pub async fn find_hevc_videos(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg/ffprobe，无法识别编码。请安装 ffmpeg 并加入 PATH。".into());
    }
    let jobs: Vec<(String, String)> = {
        let conn = state.db.lock().map_err_str()?;
        db::videos_without_codec(&conn)
            .map_err_str()?
            .into_iter()
            .filter(|(_, p)| Path::new(p).exists())
            .collect()
    };
    let total = jobs.len();
    let task_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Manager;
        let state = task_app.state::<AppState>();
        for (index, (id, path)) in jobs.into_iter().enumerate() {
            let codec = scanner::video_codec_name(&path).unwrap_or_else(|| "unknown".into());
            if let Ok(conn) = state.db.lock() {
                let _ = db::set_video_codec(&conn, &id, &codec);
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
    })
    .await
    .map_err_str()?;
    let ids = {
        let conn = state.db.lock().map_err_str()?;
        db::hevc_video_ids(&conn).map_err_str()?
    };
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
        let conn = state.db.lock().map_err_str()?;
        let mut out = Vec::new();
        for id in video_ids {
            if let Ok(Some(path)) = db::get_video_path(&conn, &id) {
                out.push((id, path));
            }
        }
        out
    };
    let transcoded_dir = app.path().app_data_dir().map_err_str()?.join("transcoded");
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
    .map_err_str()?;

    {
        let conn = state.db.lock().map_err_str()?;
        for (id, path, size) in &done {
            db::update_video_location(&conn, id, path, *size).map_err_str()?;
            db::set_video_codec(&conn, id, "h264").map_err_str()?;
        }
    }
    let _ = app.emit(
        "hevc-progress",
        HevcProgress { processed: total, total, done: true, converted: done.len(), failed: errors.len() },
    );
    Ok(ConversionResult { converted: done.len(), errors })
}

/// 重复视频的进度事件负载（事件名 video-duplicate-progress）。
/// 两趟各报一次：`anchor` 补算锚点帧指纹，`verify` 给候选补抽中段/结尾两处复核帧。
#[derive(Clone, serde::Serialize)]
struct DuplicateProgress {
    processed: usize,
    total: usize,
    stage: &'static str,
}

/// 每个采样点的双指纹允许的最大汉明距离（pHash、dHash 各自都要过这条线）。
/// 视频重压制会实打实改动像素，不能像图片那样要求指纹全等；但锚点、中段、结尾
/// 三处都得同时过线，单点放宽到 6 在三处叠起来之后误判概率已经很低。
const DUP_FRAME_DIST: u32 = 6;

/// 某一处采样点是否算"同一幅画面"
fn frames_match(a: (u64, u64), b: (u64, u64)) -> bool {
    scanner::hamming_distance(a.0, b.0) <= DUP_FRAME_DIST
        && scanner::hamming_distance(a.1, b.1) <= DUP_FRAME_DIST
}

/// 并查集分桶：把下标两两按 `same` 并组，返回每组下标。
/// 这里只用来圈候选，链式并组（A≈B、B≈C 但 A≉C）不要紧——
/// 真正定组的是后面拿三处采样点逐一核对的那一趟。
fn cluster_by(len: usize, same: impl Fn(usize, usize) -> bool) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..len).collect();
    fn find(parent: &mut Vec<usize>, i: usize) -> usize {
        if parent[i] != i {
            let root = find(parent, parent[i]);
            parent[i] = root;
        }
        parent[i]
    }
    for a in 0..len {
        for b in (a + 1)..len {
            if same(a, b) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra] = rb;
                }
            }
        }
    }
    let mut buckets: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..len {
        buckets.entry(find(&mut parent, i)).or_default().push(i);
    }
    buckets.into_values().filter(|bucket| bucket.len() > 1).collect()
}

/// 以"最早添加的那条"为保留原件，逐个拿三处采样点核对：过线的进组，
/// 过不了线的留给下一轮另起一组（所以一组内任意两条都是直接比对过的）。
fn group_by_frames(entries: &[(String, String, [(u64, u64); 3])]) -> Vec<Vec<String>> {
    let mut pool: Vec<(String, String, [(u64, u64); 3])> = entries.to_vec();
    pool.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mut groups: Vec<Vec<String>> = Vec::new();
    while !pool.is_empty() {
        let keeper = pool.remove(0);
        let mut group = vec![keeper.0.clone()];
        pool.retain(|entry| {
            let matched = (0..3).all(|slot| frames_match(keeper.2[slot], entry.2[slot]));
            if matched {
                group.push(entry.0.clone());
            }
            !matched
        });
        if group.len() > 1 {
            groups.push(group);
        }
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.len()));
    groups
}

/// 只对还缺锚点帧的行跑 ffmpeg，攒一批落一次库并推一次进度。
/// 锚点统一从缩略图派生（32×32 缩略是同一条口径），没有缩略图的先补一张。
fn ensure_anchors(
    app: &tauri::AppHandle,
    rows: &[(db::VideoSig, String)],
    indexes: &[usize],
    thumb_dir: &Path,
    anchors: &mut [Option<(u64, u64)>],
) {
    const EMIT_EVERY: usize = 128;
    let total = indexes.len();
    let _ = app.emit("video-duplicate-progress", DuplicateProgress { processed: 0, total, stage: "anchor" });
    let mut fresh: Vec<(String, i64, i64, String)> = Vec::with_capacity(EMIT_EVERY);
    for (processed, &index) in indexes.iter().enumerate() {
        let (row, mtime) = &rows[index];
        let source = match row.thumbnail_path.as_deref() {
            Some(thumb) if Path::new(thumb).exists() => Some(thumb.to_string()),
            _ => {
                // 抽帧落到缩略图目录并登记：这趟活儿顺手把缺的封面补齐，
                // 下次「生成缩略图」就不必再为这条重复解码。
                let out = thumb_dir.join(format!("{}.jpg", row.id));
                match scanner::extract_thumbnail(&row.path, &out, row.duration) {
                    Ok(()) => {
                        if let Ok(conn) = app.state::<AppState>().db.lock() {
                            let _ = db::set_thumbnail(&conn, &row.id, &out.to_string_lossy());
                        }
                        Some(out.to_string_lossy().to_string())
                    }
                    Err(_) => None,
                }
            }
        };
        if let Some((phash, dhash)) = source.as_deref().and_then(scanner::image_frame_hashes) {
            anchors[index] = Some((phash, dhash));
            fresh.push((row.id.clone(), phash as i64, dhash as i64, mtime.clone()));
        }
        let done = processed + 1;
        if done % EMIT_EVERY == 0 || done == total {
            flush(app, &mut fresh, |conn, rows| db::save_video_anchors(conn, rows));
            let _ = app.emit("video-duplicate-progress", DuplicateProgress { processed: done, total, stage: "anchor" });
        }
    }
    flush_with_retry(app, &mut fresh, |conn, rows| db::save_video_anchors(conn, rows));
}

// flush_anchors / retry_flush / flush_frames 已删，改用 commands.rs 共享的 flush / flush_with_retry

/// 候选才走的第二趟：每个时间点一次 ffmpeg 快进定位（约 0.3s/处），
/// 所以这一趟的代价只落在锚点对得上的那几条上。
fn ensure_frames(
    app: &tauri::AppHandle,
    rows: &[(db::VideoSig, String)],
    indexes: &[usize],
    frames: &mut [Option<[(u64, u64); 2]>],
) {
    const EMIT_EVERY: usize = 16;
    let total = indexes.len();
    let _ = app.emit("video-duplicate-progress", DuplicateProgress { processed: 0, total, stage: "verify" });
    let mut fresh: Vec<(String, i64, i64, i64, i64, String)> = Vec::with_capacity(EMIT_EVERY);
    for (processed, &index) in indexes.iter().enumerate() {
        let (row, mtime) = &rows[index];
        let [mid, tail] = scanner::video_sample_times(row.duration);
        let picked = scanner::video_frame_hashes(&row.path, mid)
            .zip(scanner::video_frame_hashes(&row.path, tail));
        if let Some(pair) = picked {
            frames[index] = Some([pair.0, pair.1]);
            fresh.push((
                row.id.clone(),
                pair.0 .0 as i64,
                pair.0 .1 as i64,
                pair.1 .0 as i64,
                pair.1 .1 as i64,
                mtime.clone(),
            ));
        }
        let done = processed + 1;
        if done % EMIT_EVERY == 0 || done == total {
            flush(app, &mut fresh, |conn, rows| db::save_video_frames(conn, rows));
            let _ = app.emit("video-duplicate-progress", DuplicateProgress { processed: done, total, stage: "verify" });
        }
    }
    flush_with_retry(app, &mut fresh, |conn, rows| db::save_video_frames(conn, rows));
}

/// 找出画面相同的重复视频组：不再比文件字节（重压制、重封装过就漏判），
/// 改成三处采样点的画面指纹——锚点用已有缩略图（10% 处），中段和结尾
/// 各抽一帧（50%、85% 处）。三处都对得上才算同一部片子。
/// 每组按添加时间升序（第一个是要保留的原件），大的组排前面。
#[tauri::command]
pub async fn find_duplicate_videos(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Vec<String>>, String> {
    // 判据换成解码画面之后，ffmpeg 成了硬依赖（读文件字节不需要它）
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法比对视频画面。请安装 ffmpeg 并加入 PATH。".into());
    }
    // 磁盘上已经不存在的条目不参与判定；mtime 同时当指纹缓存的钥匙用
    let all_rows: Vec<db::VideoSig> = {
        let conn = state.db.lock().map_err_str()?;
        db::get_video_sigs(&conn).map_err_str()?
    };
    // 缓存的过期判断按"库里的记录数"，所以要在丢掉读不到 mtime 的那些之前数
    let library_count = all_rows.len();
    let rows: Vec<(db::VideoSig, String)> = all_rows
        .into_iter()
        .filter_map(|row| {
            let mtime = scanner::modified_stamp(Path::new(&row.path))?;
            Some((row, mtime))
        })
        .collect();
    let thumb_dir = app.path().app_data_dir().map_err_str()?.join("thumbnails");

    let groups = tauri::async_runtime::spawn_blocking(move || {
        let _ = std::fs::create_dir_all(&thumb_dir);
        let mut anchors: Vec<Option<(u64, u64)>> = Vec::with_capacity(rows.len());
        let mut frames: Vec<Option<[(u64, u64); 2]>> = Vec::with_capacity(rows.len());
        let mut need_anchor: Vec<usize> = Vec::new();
        for (index, (row, mtime)) in rows.iter().enumerate() {
            match row.cached_anchor(mtime) {
                Some(anchor) => {
                    anchors.push(Some(anchor));
                    frames.push(row.cached_frames(mtime));
                }
                None => {
                    anchors.push(None);
                    frames.push(None);
                    need_anchor.push(index);
                }
            }
        }
        ensure_anchors(&app, &rows, &need_anchor, &thumb_dir, anchors.as_mut_slice());

        let bucketed: Vec<usize> = (0..rows.len()).filter(|&i| anchors[i].is_some()).collect();
        let bucket_anchors = |slot: usize, other: usize| {
            matches!((anchors[bucketed[slot]], anchors[bucketed[other]]),
                (Some(a), Some(b)) if frames_match(a, b))
        };
        let candidates: Vec<usize> = cluster_by(bucketed.len(), bucket_anchors)
            .into_iter()
            .flatten()
            .map(|slot| bucketed[slot])
            .collect();

        let need_frames: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| frames[i].is_none())
            .collect();
        ensure_frames(&app, &rows, &need_frames, frames.as_mut_slice());

        let judged: Vec<(String, String, [(u64, u64); 3])> = candidates
            .iter()
            .filter_map(|&i| {
                let anchor = anchors[i]?;
                let [mid, tail] = frames[i]?;
                Some((rows[i].0.id.clone(), rows[i].0.created_at.clone(), [anchor, mid, tail]))
            })
            .collect();
        let groups = group_by_frames(&judged);
        // 这一趟热跑也要几分钟，落一份缓存，重启后直接接着看
        write_cache(
            &app,
            VIDEO_DUPLICATE_CACHE,
            &VideoDuplicateCache {
                library_count,
                groups: groups.clone(),
            },
        );
        groups
    })
    .await
    .map_err_str()?;

    Ok(groups)
}

/// 重复视频检测的落盘缓存：`<app_data>/video-duplicate-cache.json`。
/// 与图片侧同一个理由：跑一趟要等几分钟，重启后先把上一趟的组恢复出来看。
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDuplicateCache {
    pub library_count: usize,
    pub groups: Vec<Vec<String>>,
}

/// 上一趟的重复视频结果（没有则 null）。恢复出来的名单会照现在的库裁一遍再显示
#[tauri::command]
pub async fn get_video_duplicate_cache(
    app: tauri::AppHandle,
) -> Result<Option<VideoDuplicateCache>, String> {
    read_cache(&app, VIDEO_DUPLICATE_CACHE).await
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
        let conn = state.db.lock().map_err_str()?;
        db::get_video_path(&conn, &video_id)
            .map_err_str()?
            .ok_or_else(|| format!("Video not found: {}", video_id))?
    };

    let probe_path = path.clone();
    let codec = tauri::async_runtime::spawn_blocking(move || scanner::video_codec_name(&probe_path))
        .await
        .map_err_str()?;
    if codec.as_deref() != Some("hevc") {
        return Ok(path);
    }

    let dir = app.path().app_data_dir().map_err_str()?.join("transcoded");
    std::fs::create_dir_all(&dir).map_err_str()?;
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
        .map_err_str()?;
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

    /// 把最低位的 n 个 bit 翻掉，用来精确构造汉明距离
    fn flip(value: u64, n: u32) -> u64 {
        value ^ if n == 0 { 0 } else { (1u64 << n) - 1 }
    }

    #[test]
    fn test_frames_match_needs_both_hashes_close() {
        assert!(frames_match((0, 0), (flip(0, DUP_FRAME_DIST), flip(0, DUP_FRAME_DIST))));
        assert!(!frames_match((0, 0), (flip(0, DUP_FRAME_DIST + 1), 0)), "pHash 超线就不算同一画面");
        assert!(!frames_match((0, 0), (0, flip(0, DUP_FRAME_DIST + 1))), "dHash 超线就不算同一画面");
    }

    /// 锚点是从缩略图（480px JPEG）派生的，而复核帧直接取自源文件：
    /// 缩略图这一路必须也能认出"压制参数不同的同一部片子"，否则候选都圈不出来。
    #[test]
    fn test_anchor_from_thumbnail_survives_reencoding() {
        if !scanner::ffmpeg_available() {
            eprintln!("跳过：本机未安装 ffmpeg");
            return;
        }
        let dir = std::env::temp_dir().join(format!("viewman-anchor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let render = |name: &str, source: &str, extra: &[&str]| {
            let out = dir.join(name);
            let ok = std::process::Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", source, "-pix_fmt", "yuv420p"])
                .args(extra)
                .arg(&out)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "生成测试视频失败");
            out.to_string_lossy().to_string()
        };
        let pattern = "testsrc=size=640x480:rate=25:duration=6";
        let original = render("orig.mp4", pattern, &["-b:v", "2M"]);
        let reencoded = render("again.mp4", pattern, &["-vf", "scale=320:240", "-b:v", "250k"]);
        let other = render("other.mp4", "testsrc2=size=640x480:rate=25:duration=6", &["-b:v", "2M"]);

        let anchor = |video: &str, name: &str| {
            let thumb = dir.join(name);
            scanner::extract_thumbnail(video, &thumb, Some(6.0)).unwrap();
            scanner::image_frame_hashes(&thumb.to_string_lossy()).expect("缩略图该能取出锚点指纹")
        };
        let (a, b, c) = (anchor(&original, "a.jpg"), anchor(&reencoded, "b.jpg"), anchor(&other, "c.jpg"));
        assert!(frames_match(a, b), "重压制过的同一部片子，锚点指纹距离超出 {DUP_FRAME_DIST} 就会漏判");
        assert!(!frames_match(a, c), "另一部片子也对得上的话，候选桶就等于是没判据");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_group_by_frames_requires_all_three_slots() {
        let same = [(0u64, 0u64); 3];
        // 只有中段差一点：三处都得过线，所以这一条不配组
        let mid_off = [(0, 0), (flip(0, 20), 0), (0, 0)];
        let entries: Vec<(String, String, [(u64, u64); 3])> = vec![
            ("v1".into(), "1".into(), same),
            ("v2".into(), "2".into(), [(flip(0, 4), flip(0, 4)), same[1], same[2]]),
            ("v3".into(), "3".into(), mid_off),
            ("v4".into(), "4".into(), mid_off),
        ];
        assert_eq!(group_by_frames(&entries), vec![vec!["v1".to_string(), "v2".to_string()], vec!["v3".to_string(), "v4".to_string()]]);
    }

    #[test]
    fn test_group_by_frames_keeps_the_earliest_added_as_keeper() {
        let entries: Vec<(String, String, [(u64, u64); 3])> = vec![
            ("late".into(), "2024".into(), [(0, 0); 3]),
            ("early".into(), "2020".into(), [(0, 0); 3]),
        ];
        // 第一个是删除时保留的那条，必须是最早添加的
        assert_eq!(group_by_frames(&entries), vec![vec!["early".to_string(), "late".to_string()]]);
    }

    /// 候选桶是并查集圈出来的，允许链式相连；真正定组要逐条和保留原件比
    #[test]
    fn test_group_by_frames_does_not_inherit_chain_similarity() {
        let tail = |bits: u32| [(0u64, 0u64), (0, 0), (flip(0, bits), 0)];
        let entries: Vec<(String, String, [(u64, u64); 3])> = vec![
            ("a".into(), "1".into(), tail(0)),
            ("b".into(), "2".into(), tail(4)),
            ("c".into(), "3".into(), tail(8)),
        ];
        assert!(frames_match(entries[0].2[2], entries[1].2[2]), "a 和 b 该对得上");
        assert!(frames_match(entries[1].2[2], entries[2].2[2]), "b 和 c 该对得上");
        assert!(!frames_match(entries[0].2[2], entries[2].2[2]), "但 a 和 c 对不上");
        // c 只跟中间的 b 像，跟保留的 a 不像 → 不能靠 b 蹭进组
        assert_eq!(group_by_frames(&entries), vec![vec!["a".to_string(), "b".to_string()]]);
    }

    /// 候选桶按锚点的邻近关系并组：落单的不开桶，链式的允许并进来
    #[test]
    fn test_cluster_by_buckets_only_near_matches() {
        let sigs = [0u64, 0b1, 0b11, 0b1111_1111_1111];
        let clusters = cluster_by(sigs.len(), |i, j| {
            scanner::hamming_distance(sigs[i], sigs[j]) <= 3
        });
        assert_eq!(clusters, vec![vec![0, 1, 2]], "前三条并成一桶，第四条落单不成组");
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
