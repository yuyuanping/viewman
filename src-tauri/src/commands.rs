use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::models::{RecentlyPlayed, Video, VideoFileStatus, VideoProgress, WatchProgress};
use crate::potplayer;
use crate::scanner;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
}

#[derive(Clone, serde::Serialize)]
pub struct ScanProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct ThumbnailProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    pub generated: usize,
    pub failed: usize,
}

/// 缩略图缓存目录：`<app_data>/thumbnails`
fn thumbnails_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("thumbnails");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建缩略图目录失败: {}", e))?;
    Ok(dir)
}

#[tauri::command]
pub fn get_videos(state: State<AppState>) -> Result<Vec<Video>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_all_videos(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn scan_directory(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<Vec<Video>, String> {
    let existing_videos = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_all_videos(&conn).map_err(|e| e.to_string())?
    };

    let dir_path = std::path::PathBuf::from(&dir);
    let dir_for_task = dir.clone();
    let task_app = app.clone();
    let (videos, stale_ids, warnings) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<Video>, Vec<String>, Vec<String>), String> {
            // 目录打不开时直接报错，绝不把"没扫到"当成"已删除"去清库
            let files = match scanner::scan_directory_recursive(&dir_path) {
                Ok(f) => f,
                Err(e) => return Err(format!("扫描中断，未修改数据库：{}", e)),
            };

            let found: HashSet<String> = files
                .iter()
                .map(|f| f.to_string_lossy().to_lowercase())
                .collect();
            let existing: Vec<_> = existing_videos.iter()
                .map(|v| (v.id.clone(), v.path.clone())).collect();
            let by_path: std::collections::HashMap<_, _> = existing_videos.iter()
                .map(|v| (v.path.to_lowercase(), v)).collect();

            // 本次扫描已找不到、但库里还挂在该目录下的文件 → 视为外部已删除
            let prefix = format!("{}\\", dir_for_task.trim_end_matches('\\').to_lowercase());
            let stale_ids = compute_stale_ids(&existing, &prefix, &found);

            let new_files: Vec<_> = files
                .into_iter()
                .filter(|f| {
                    match by_path.get(&f.to_string_lossy().to_lowercase()) {
                        None => true,
                        Some(old) => old.duration.is_none() || old.width.is_none() || old.height.is_none()
                            || std::fs::metadata(f).map(|m| m.len() as i64 != old.file_size).unwrap_or(true),
                    }
                })
                .collect();

            let _ = task_app.emit(
                "scan-progress",
                ScanProgress { processed: 0, total: new_files.len(), done: false, warnings: vec![] },
            );

            let (mut videos, warnings) = build_videos_parallel(&task_app, &new_files);
            for video in &mut videos {
                if let Some(old) = by_path.get(&video.path.to_lowercase()) {
                    let incomplete = video.duration.is_none() || video.width.is_none() || video.height.is_none();
                    video.id = old.id.clone();
                    video.path = old.path.clone();
                    video.created_at = old.created_at.clone();
                    video.duration = video.duration.or(old.duration);
                    video.width = video.width.or(old.width);
                    video.height = video.height.or(old.height);
                    // A failed refresh must remain eligible for retry on the next scan.
                    if incomplete {
                        video.file_size = old.file_size;
                    }
                }
            }
            Ok((videos, stale_ids, warnings))
        })
        .await
        .map_err(|e| e.to_string())??;

    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        // 清理失效条目 + 插入新条目必须在同一事务里：要么全部生效，要么全部回滚
        // （Transaction 会解引用为 Connection，直接复用现有函数）
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        if !stale_ids.is_empty() {
            db::delete_videos_by_ids(&tx, &stale_ids).map_err(|e| e.to_string())?;
        }
        for video in &videos {
            db::insert_video(&tx, video).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }

    let _ = app.emit(
        "scan-progress",
        ScanProgress { processed: videos.len(), total: videos.len(), done: true, warnings: warnings.clone() },
    );

    Ok(videos)
}

/// 返回 should-delete 的 video id：路径在 prefix 目录下（大小写不敏感），且不在 found 集合里。
/// prefix 已含结尾分隔符，避免 "D:\v" 误匹配 "D:\vids2"。
fn compute_stale_ids(
    existing: &[(String, String)],
    prefix: &str,
    found_lower: &HashSet<String>,
) -> Vec<String> {
    existing
        .iter()
        .filter(|(_, path)| {
            let lp = path.to_lowercase();
            lp.starts_with(prefix) && !found_lower.contains(&lp)
        })
        .map(|(id, _)| id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::compute_stale_ids;

    #[test]
    fn test_compute_stale_ids() {
        let existing = vec![
            ("a".to_string(), r"D:\vid\one.mp4".to_string()),
            ("b".to_string(), r"D:\vid\sub\two.mp4".to_string()),
            ("c".to_string(), r"D:\vids2\three.mp4".to_string()),
            ("d".to_string(), r"C:\other\four.mp4".to_string()),
        ];
        // 目录里现在只有 one.mp4（大小写不同也要识别为存在）和 vids2 下的 three.mp4
        let found: std::collections::HashSet<String> = [r"D:\vid\ONE.mp4", r"D:\vids2\three.mp4"]
            .iter()
            .map(|s| s.to_lowercase())
            .collect();

        let stale = compute_stale_ids(&existing, r"d:\vid\", &found);
        assert_eq!(stale, vec!["b".to_string()]);
    }

    #[test]
    fn test_compute_stale_ids_prefix_boundary() {
        // "D:\v" 不能误匹配 "D:\vids2\x.mp4"
        let existing = vec![("c".to_string(), r"D:\vids2\three.mp4".to_string())];
        let found: std::collections::HashSet<String> = std::collections::HashSet::new();
        let stale = compute_stale_ids(&existing, r"d:\v\", &found);
        assert!(stale.is_empty());
    }
}

fn build_videos_parallel(
    app: &tauri::AppHandle,
    files: &[std::path::PathBuf],
) -> (Vec<Video>, Vec<String>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let total = files.len();
    let next = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let probe_failures = AtomicUsize::new(0);
    let thread_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .min(total);

    let videos = std::thread::scope(|s| {
        let handles: Vec<_> = (0..thread_count)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        let video = scanner::build_video(&files[i]);
                        if video.duration.is_none() {
                            probe_failures.fetch_add(1, Ordering::Relaxed);
                        }
                        out.push(video);
                        let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = app.emit(
                            "scan-progress",
                            ScanProgress { processed: done, total, done: false, warnings: vec![] },
                        );
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });

    let mut warnings = Vec::new();
    let failures = probe_failures.load(Ordering::Relaxed);
    if failures > 0 {
        warnings.push(format!(
            "{} 个视频未能获取时长（请确认已安装 ffmpeg/ffprobe，或文件本身损坏）",
            failures
        ));
    }
    (videos, warnings)
}

#[tauri::command]
pub fn save_progress(state: State<AppState>, video_id: String, position: f64) -> Result<(), String> {
    if !position.is_finite() || position < 0.0 {
        return Err(format!("非法的播放位置: {}", position));
    }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::upsert_progress(&conn, &video_id, position).map_err(|e| e.to_string())
}

/// 只打开文件句柄确认可读性，不读取视频内容
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
        Ok(_) => Ok(VideoFileStatus { status: "readable".into(), message: None }),
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
pub fn get_progress(state: State<AppState>, video_id: String) -> Result<Option<WatchProgress>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_progress(&conn, &video_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_videos_with_progress(state: State<AppState>) -> Result<Vec<VideoProgress>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_videos_with_progress(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_recently_played(state: State<AppState>) -> Result<Vec<RecentlyPlayed>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_recently_played(&conn, 30).map_err(|e| e.to_string())
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

    // 封面缓存文件名由视频 id 决定，删除视频时一并清理，避免留下孤儿文件
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join("thumbnails").join(format!("{}.jpg", video_id)));
    }
    Ok(())
}

const SCAN_ROOTS_KEY: &str = "scan_roots";

/// 读取已记录的扫描目录清单（存于数据库，dev/正式版共享，不受 WebView 存储隔离影响）
#[tauri::command]
pub fn load_scan_roots(state: State<AppState>) -> Result<Vec<String>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let raw = db::get_setting(&conn, SCAN_ROOTS_KEY).map_err(|e| e.to_string())?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default())
}

#[tauri::command]
pub fn save_scan_roots(state: State<AppState>, roots: Vec<String>) -> Result<(), String> {
    let value = serde_json::to_string(&roots).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::set_setting(&conn, SCAN_ROOTS_KEY, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_ffprobe() -> bool {
    scanner::hidden_command("ffprobe")
        .arg("-version")
        .output()
        .is_ok()
}

#[tauri::command]
pub fn check_potplayer() -> bool {
    potplayer::find_potplayer().is_some()
}

#[tauri::command]
pub fn launch_potplayer(video_path: String, seek: Option<f64>) -> Result<(), String> {
    potplayer::launch(&video_path, seek)
}

#[tauri::command]
pub fn enable_potplayer_titlebar() -> Result<(), String> {
    potplayer::enable_titlebar_time()
}

#[tauri::command]
pub fn potplayer_status(video_path: String) -> potplayer::PotPlayerStatus {
    potplayer::get_status(&video_path)
}

/// 为指定视频生成封面（ffmpeg 抽帧），已有有效缓存文件的会跳过。
/// 抽帧过程不持有数据库锁：先在后台线程生成文件，再统一写库。
#[tauri::command]
pub async fn generate_thumbnails(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    video_ids: Vec<String>,
) -> Result<usize, String> {
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法生成封面。请安装 ffmpeg 并加入 PATH。".into());
    }

    let dir = thumbnails_dir(&app)?;

    let jobs: Vec<Video> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let all = db::get_all_videos(&conn).map_err(|e| e.to_string())?;
        all.into_iter()
            .filter(|v| video_ids.iter().any(|id| id == &v.id))
            // 缓存文件仍在的跳过；文件被删掉的会重新生成
            .filter(|v| {
                !matches!(&v.thumbnail_path, Some(p) if std::path::Path::new(p).exists())
            })
            .collect()
    };

    let total = jobs.len();
    if total == 0 {
        let _ = app.emit(
            "thumbnail-progress",
            ThumbnailProgress { processed: 0, total: 0, done: true, generated: 0, failed: 0 },
        );
        return Ok(0);
    }

    let task_app = app.clone();
    let (done, failed) = tauri::async_runtime::spawn_blocking(move || {
        let mut done: Vec<(String, String)> = Vec::new();
        let mut failed = 0usize;
        for (index, video) in jobs.iter().enumerate() {
            let out = dir.join(format!("{}.jpg", video.id));
            match scanner::extract_thumbnail(&video.path, &out, video.duration) {
                Ok(()) => done.push((video.id.clone(), out.to_string_lossy().to_string())),
                Err(_) => failed += 1,
            }
            let _ = task_app.emit(
                "thumbnail-progress",
                ThumbnailProgress {
                    processed: index + 1,
                    total,
                    done: false,
                    generated: done.len(),
                    failed,
                },
            );
        }
        (done, failed)
    })
    .await
    .map_err(|e| e.to_string())?;

    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        for (id, path) in &done {
            db::set_thumbnail(&conn, id, path).map_err(|e| e.to_string())?;
        }
    }

    let _ = app.emit(
        "thumbnail-progress",
        ThumbnailProgress { processed: total, total, done: true, generated: done.len(), failed },
    );

    Ok(done.len())
}
