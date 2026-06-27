use tauri::State;
use crate::db;
use crate::models::{RecentlyPlayed, Video, VideoProgress, WatchProgress};
use crate::scanner;
use std::sync::Mutex;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
}

#[tauri::command]
pub fn get_videos(state: State<AppState>) -> Result<Vec<Video>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_all_videos(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn scan_directory(state: State<AppState>, dir: String) -> Result<Vec<Video>, String> {
    let dir_path = std::path::Path::new(&dir);
    let files = scanner::scan_directory_recursive(dir_path)?;

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let existing_paths: Vec<String> = db::get_existing_paths(&conn).map_err(|e| e.to_string())?;

    let mut new_videos = Vec::new();
    for file in &files {
        let path_str = file.to_string_lossy().to_string();
        if existing_paths.contains(&path_str) {
            continue;
        }
        let video = scanner::build_video(file);
        db::insert_video(&conn, &video).map_err(|e| e.to_string())?;
        new_videos.push(video);
    }

    Ok(new_videos)
}

#[tauri::command]
pub fn save_progress(state: State<AppState>, video_id: String, position: f64) -> Result<(), String> {
    eprintln!("[save_progress] video_id={}, position={}", video_id, position);
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let before: i64 = conn.query_row("SELECT COUNT(*) FROM watch_progress", [], |row| row.get(0)).unwrap_or(-1);
    db::upsert_progress(&conn, &video_id, position).map_err(|e| e.to_string())?;
    let after: i64 = conn.query_row("SELECT COUNT(*) FROM watch_progress", [], |row| row.get(0)).unwrap_or(-1);
    eprintln!("[save_progress] watch_progress count: {} -> {}", before, after);
    Ok(())
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
    eprintln!("[get_recently_played] called");
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM watch_progress", [], |row| row.get(0)).unwrap_or(-1);
    eprintln!("[get_recently_played] total watch_progress records: {}", total);
    let result = db::get_recently_played(&conn, 30).map_err(|e| e.to_string())?;
    eprintln!("[get_recently_played] returning {} records", result.len());
    for r in &result {
        eprintln!("[get_recently_played]   video_id={}, filename={}, updated_at={}", r.video.id, r.video.filename, r.updated_at);
    }
    Ok(result)
}

#[tauri::command]
pub fn check_ffprobe() -> bool {
    std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_ok()
}
