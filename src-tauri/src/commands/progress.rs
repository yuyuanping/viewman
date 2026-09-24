use tauri::State;

use crate::db;
use crate::models::{RecentlyPlayed, VideoProgress, WatchProgress};

use super::AppState;

#[tauri::command]
pub fn save_progress(state: State<AppState>, video_id: String, position: f64) -> Result<(), String> {
    if !position.is_finite() || position < 0.0 {
        return Err(format!("非法的播放位置: {}", position));
    }
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

/// 完整播放历史：watch_progress 全表按时间倒序（侧栏"播放记录"只有最近 30 条）
#[tauri::command]
pub fn get_play_history(state: State<AppState>) -> Result<Vec<RecentlyPlayed>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_recently_played(&conn, i64::MAX).map_err(|e| e.to_string())
}
