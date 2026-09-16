use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, State};

use crate::db;
use crate::models::{RecentlyPlayed, Video, VideoProgress, WatchProgress};
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
    let existing: Vec<(String, String)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_video_paths(&conn).map_err(|e| e.to_string())?
    };

    let dir_path = std::path::PathBuf::from(&dir);
    let dir_for_task = dir.clone();
    let task_app = app.clone();
    let (videos, stale_ids) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<Video>, Vec<String>), String> {
            let files = scanner::scan_directory_recursive(&dir_path)?;

            let found: HashSet<String> = files
                .iter()
                .map(|f| f.to_string_lossy().to_lowercase())
                .collect();
            let existing_lc: HashSet<String> =
                existing.iter().map(|(_, p)| p.to_lowercase()).collect();

            // 本次扫描已找不到、但库里还挂在该目录下的文件 → 视为外部已删除
            let prefix = format!("{}\\", dir_for_task.trim_end_matches('\\').to_lowercase());
            let stale_ids = compute_stale_ids(&existing, &prefix, &found);

            let new_files: Vec<_> = files
                .into_iter()
                .filter(|f| !existing_lc.contains(&f.to_string_lossy().to_lowercase()))
                .collect();

            let _ = task_app.emit(
                "scan-progress",
                ScanProgress { processed: 0, total: new_files.len(), done: false },
            );

            Ok((build_videos_parallel(&task_app, &new_files), stale_ids))
        })
        .await
        .map_err(|e| e.to_string())??;

    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        if !stale_ids.is_empty() {
            db::delete_videos_by_ids(&conn, &stale_ids).map_err(|e| e.to_string())?;
        }
        for video in &videos {
            db::insert_video(&conn, video).map_err(|e| e.to_string())?;
        }
    }

    let _ = app.emit(
        "scan-progress",
        ScanProgress { processed: videos.len(), total: videos.len(), done: true },
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

fn build_videos_parallel(app: &tauri::AppHandle, files: &[std::path::PathBuf]) -> Vec<Video> {
    if files.is_empty() {
        return Vec::new();
    }

    let total = files.len();
    let next = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let thread_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .min(total);

    std::thread::scope(|s| {
        let handles: Vec<_> = (0..thread_count)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        out.push(scanner::build_video(&files[i]));
                        let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = app.emit(
                            "scan-progress",
                            ScanProgress { processed: done, total, done: false },
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
    })
}

#[tauri::command]
pub fn save_progress(state: State<AppState>, video_id: String, position: f64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::upsert_progress(&conn, &video_id, position).map_err(|e| e.to_string())
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
pub fn delete_video(state: State<AppState>, video_id: String) -> Result<(), String> {
    let path = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_video_path(&conn, &video_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Video not found: {}", video_id))?
    };

    trash::delete(&path).map_err(|e| format!("删除文件失败: {}", e))?;

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::delete_video(&conn, &video_id).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn check_ffprobe() -> bool {
    std::process::Command::new("ffprobe")
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
