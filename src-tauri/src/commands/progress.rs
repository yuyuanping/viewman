use tauri::{Manager, State};

use crate::db;
use crate::models::{RecentlyPlayed, VideoProgress};

use super::{AppState, MapErrStr};

#[tauri::command]
pub fn save_progress(state: State<AppState>, video_id: String, position: f64) -> Result<(), String> {
    if !position.is_finite() || position < 0.0 {
        return Err(format!("非法的播放位置: {}", position));
    }
    let conn = state.db.lock().map_err_str()?;
    // 先确认视频还在库里：FK 违规的原始报错换成用户能看懂的提示
    if db::get_video_path(&conn, &video_id)
        .map_err_str()?
        .is_none()
    {
        return Err(format!("视频不存在或已被删除: {}", video_id));
    }
    db::upsert_progress(&conn, &video_id, position).map_err_str()
}

#[tauri::command]
pub async fn get_videos_with_progress(app: tauri::AppHandle) -> Result<Vec<VideoProgress>, String> {
    // 全表 LEFT JOIN + 整表下发，启动时就要跑：序列化在阻塞线程池做，主线程只管回传
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        db::get_videos_with_progress(&conn).map_err_str()
    })
    .await
    .map_err_str()?
}

#[tauri::command]
pub async fn get_recently_played(app: tauri::AppHandle) -> Result<Vec<RecentlyPlayed>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        db::get_recently_played(&conn, 30).map_err_str()
    })
    .await
    .map_err_str()?
}

/// 完整播放历史：watch_progress 全表按时间倒序（侧栏"播放记录"只有最近 30 条）。
/// 全表 join + 序列化是秒级开销，扔进阻塞线程池，别堵主线程。
#[tauri::command]
pub async fn get_play_history(app: tauri::AppHandle) -> Result<Vec<RecentlyPlayed>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        db::get_recently_played(&conn, i64::MAX).map_err_str()
    })
    .await
    .map_err_str()?
}
