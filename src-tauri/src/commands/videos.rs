use std::io::Read;
use std::path::{Path, PathBuf};

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::models::{ConversionResult, Image, Video, VideoFileStatus};
use crate::scanner;

use super::thumbnails::clear_thumbnail_cache;
use super::{undeleted_targets, video_path_or, AppState, MapErrStr};

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
/// 耗尽后报错而不是退回原路径——原路径必然已存在，调用方会直接覆盖文件
fn unique_path(target: &Path) -> Result<PathBuf, String> {
    if !target.exists() {
        return Ok(target.to_path_buf());
    }
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let ext = target.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
    for i in 2..1000 {
        let candidate = target.with_file_name(format!("{stem} ({i}).{ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("无法生成不重名的目标路径".into())
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
        video_path_or(&conn, &video_id)?
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
    match std::fs::File::open(p) {
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
    // 库里已经没有记录的 id 视作已删除（口径同图片侧 split_known_targets），
    // 不能因为个别陈旧 id 让整批都删不动
    let (targets, mut deleted): (Vec<(String, String)>, Vec<String>) = {
        let conn = state.db.lock().map_err_str()?;
        let mut targets = Vec::with_capacity(video_ids.len());
        let mut gone = Vec::new();
        for video_id in &video_ids {
            match db::get_video_path(&conn, video_id).map_err_str()? {
                Some(path) => targets.push((video_id.clone(), path)),
                None => gone.push(video_id.clone()),
            }
        }
        (targets, gone)
    };

    let for_files = targets.clone();
    let survivors = tauri::async_runtime::spawn_blocking(move || undeleted_targets(&for_files))
        .await
        .map_err_str()?;
    let failed: std::collections::HashSet<String> = survivors.into_iter().map(|(id, _)| id).collect();
    deleted.extend(
        targets
            .iter()
            .filter(|(id, _)| !failed.contains(id))
            .map(|(id, _)| id.clone()),
    );

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
pub async fn convert_fake_images(
    app: tauri::AppHandle,
    video_ids: Vec<String>,
) -> Result<ConversionResult, String> {
    // 逐文件 fs::copy + trash::delete + ffmpeg 是重活，扔进阻塞线程池，别堵主线程
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
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

            let target = unique_path(&p.with_extension(ext))?;
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
    })
    .await
    .map_err_str()?
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
                let target = match unique_path(&p.with_extension(ext)) {
                    Ok(t) => t,
                    Err(e) => {
                        errors.push(format!("{}: {}", p.display(), e));
                        continue;
                    }
                };
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
            let target = match unique_path(&p.with_extension("jpg")) {
                Ok(t) => t,
                Err(e) => {
                    errors.push(format!("{}: {}", p.display(), e));
                    continue;
                }
            };
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

/// 文件移动：同盘 rename；跨盘（Windows ERROR_NOT_SAME_DEVICE）回退为复制+删源。
/// 目标已存在时直接报错而不做静默覆盖——检查与执行之间的窗口宁可失败也不吞文件。
pub(crate) fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        return Err(format!("目标文件已存在，拒绝覆盖: {}", dst.display()));
    }
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

/// target_dir 与 old_path 所在目录是否为同一目录：分隔符统一成 '\'、去掉结尾
/// 分隔符后做大小写不敏感比较——字符串直比会把带正斜杠或结尾反斜杠的写法
/// 误判成不同目录，导致同目录移动被当成正常移动执行
pub(crate) fn same_target_dir(old_path: &str, target_dir: &str) -> bool {
    let normalized = target_dir.replace('/', "\\");
    let normalized = normalized.trim_end_matches('\\');
    if normalized.is_empty() {
        return false;
    }
    let old_dir = Path::new(old_path)
        .parent()
        .map(|d| d.to_string_lossy().replace('/', "\\"))
        .unwrap_or_default();
    let old_dir = old_dir.trim_end_matches('\\');
    old_dir.eq_ignore_ascii_case(normalized)
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
        let old_path = video_path_or(&conn, &video_id)?;
        let src = Path::new(&old_path);
        let filename = src
            .file_name()
            .ok_or_else(|| "源文件路径无效".to_string())?
            .to_string_lossy()
            .to_string();
        let dir = Path::new(&target_dir);
        if same_target_dir(&old_path, &target_dir) {
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
    move_result?;

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
    // 锁内只收集行集，"文件还在不在磁盘上"的逐条 stat 挪到锁外
    let candidates: Vec<(String, String)> = {
        let conn = state.db.lock().map_err_str()?;
        db::videos_without_codec(&conn).map_err_str()?
    };
    let jobs: Vec<(String, String)> = candidates
        .into_iter()
        .filter(|(_, p)| Path::new(p).exists())
        .collect();
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
            let target = if p.exists() {
                match unique_path(p) {
                    Ok(t) => t,
                    Err(e) => {
                        errors.push(format!("{} 回写失败（{}），请从回收站还原", p.display(), e));
                        emit(converted, errors.len());
                        continue;
                    }
                }
            } else {
                p.to_path_buf()
            };
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
        video_path_or(&conn, &video_id)?
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

    // 先写同目录临时名再 rename：并发请求不会各写一路 ffmpeg 到同一个最终文件
    // 互相踩踏；成功后 rename 覆盖旧缓存是预期行为（覆盖的正是本函数刚产出的有效文件）
    let tmp = dir.join(format!("{}.tmp{}", video_id, uuid::Uuid::new_v4()));
    let src = path.clone();
    let out_str = out.to_string_lossy().to_string();
    let tmp_str = tmp.to_string_lossy().to_string();
    let dst_str = out_str.clone();
    let job = tauri::async_runtime::spawn_blocking(move || {
        scanner::transcode_to_h264(&src, Path::new(&tmp_str)).and_then(|()| {
            std::fs::rename(&tmp_str, &dst_str)
                .map_err(|e| format!("转码完成但写回缓存失败: {}", e))
        })
    })
    .await
    .map_err_str()?;
    let _ = std::fs::remove_file(&tmp); // 失败时清掉残留的临时文件
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
    fn test_unique_path_avoids_collision() {
        let dir = std::env::temp_dir().join(format!("viewman-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let taken = dir.join("a.jpg");
        std::fs::write(&taken, b"x").unwrap();

        assert_eq!(unique_path(&dir.join("b.jpg")).unwrap(), dir.join("b.jpg"));
        assert_eq!(unique_path(&taken).unwrap(), dir.join("a (2).jpg"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
