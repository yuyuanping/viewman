use tauri::State;

use crate::db;

use super::removal::normalized_dir;
use super::AppState;

pub(crate) const SCAN_ROOTS_KEY: &str = "scan_roots";
pub(crate) const IMAGE_SCAN_ROOTS_KEY: &str = "image_scan_roots";

/// 视频库与图片库各存一份目录清单，互不干扰（键名沿用旧的 scan_roots，老用户不丢记录）
pub(crate) fn roots_key(kind: &str) -> Result<&'static str, String> {
    match kind {
        "video" => Ok(SCAN_ROOTS_KEY),
        "image" => Ok(IMAGE_SCAN_ROOTS_KEY),
        other => Err(format!("未知的媒体类型: {}", other)),
    }
}

/// 读取已记录的扫描目录清单（存于数据库，dev/正式版共享，不受 WebView 存储隔离影响）
#[tauri::command]
pub fn load_scan_roots(state: State<AppState>, kind: String) -> Result<Vec<String>, String> {
    let key = roots_key(&kind)?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let raw = db::get_setting(&conn, key).map_err(|e| e.to_string())?;
    Ok(raw
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default())
}

#[tauri::command]
pub fn save_scan_roots(
    state: State<AppState>,
    kind: String,
    roots: Vec<String>,
) -> Result<(), String> {
    let key = roots_key(&kind)?;
    let value = serde_json::to_string(&roots).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::set_setting(&conn, key, &value).map_err(|e| e.to_string())
}

/// 记下扫描根（大小写与结尾分隔符不敏感去重）。
/// 只要目录能正常读取就立刻记，别等整轮扫描结束——扫到一半被打断，下次启动才能接着扫。
pub(crate) fn remember_root(conn: &rusqlite::Connection, key: &str, dir: &str) -> Result<(), String> {
    let raw = db::get_setting(conn, key).map_err(|e| e.to_string())?;
    let mut roots: Vec<String> = raw
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    if roots
        .iter()
        .any(|r| normalized_dir(r) == normalized_dir(dir))
    {
        return Ok(());
    }
    roots.push(dir.to_string());
    let value = serde_json::to_string(&roots).map_err(|e| e.to_string())?;
    db::set_setting(conn, key, &value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_and_image_roots_are_isolated() {
        assert!(roots_key("video").unwrap() == SCAN_ROOTS_KEY);
        assert_ne!(roots_key("image").unwrap(), roots_key("video").unwrap());
        assert!(roots_key("audio").is_err());
    }

    #[test]
    fn test_remember_root_records_each_directory_once() {
        let conn = db::setup_test_db();
        remember_root(&conn, IMAGE_SCAN_ROOTS_KEY, r"D:\pics").unwrap();
        // 大小写与结尾分隔符不同也算同一个目录，不该记两遍
        remember_root(&conn, IMAGE_SCAN_ROOTS_KEY, r"d:\pics\").unwrap();
        remember_root(&conn, IMAGE_SCAN_ROOTS_KEY, r"E:\media").unwrap();

        let raw = db::get_setting(&conn, IMAGE_SCAN_ROOTS_KEY)
            .unwrap()
            .unwrap();
        let roots: Vec<String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(roots, vec![r"D:\pics".to_string(), r"E:\media".to_string()]);
        // 图片清单的变动不会波及视频清单
        assert!(db::get_setting(&conn, SCAN_ROOTS_KEY).unwrap().is_none());
    }
}
