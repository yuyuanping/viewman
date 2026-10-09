use tauri::Manager;

use crate::db;

use super::{AppState, MapErrStr};

/// 数据库自检：返回 "ok"，否则返回首条损坏描述（19 万行约需几秒，阻塞线程池跑）。
#[tauri::command]
pub async fn check_db_integrity(app: tauri::AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        db::integrity_check(&conn).map_err_str()
    })
    .await
    .map_err_str()?
}

/// 无损重建 FTS 索引：主表数据不动。返回重建后的索引行数描述。
/// 删除/搜索报 malformed 且自检指向 FTS 时调这个；主表损坏则修不了，需从备份恢复。
#[tauri::command]
pub async fn rebuild_fts_indexes(app: tauri::AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        let (v, i) = db::rebuild_fts(&conn).map_err_str()?;
        Ok(format!("视频索引 {} 条，图片索引 {} 条", v, i))
    })
    .await
    .map_err_str()?
}

/// 库文件路径：主表损坏需手动恢复时告诉用户文件在哪
#[tauri::command]
pub fn db_file_path(app: tauri::AppHandle) -> Result<String, String> {
    Ok(app.state::<AppState>().db_path.clone())
}
