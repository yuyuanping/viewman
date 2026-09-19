use tauri::State;

use crate::db;

use super::AppState;

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
