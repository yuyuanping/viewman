use tauri::State;

use crate::db;

use super::settings::roots_key;
use super::thumbnails::clear_thumbnail_cache;
use super::AppState;

/// 统一比较形式：小写 + 分隔符一律按 '\' 处理（Windows 路径大小写不敏感）
fn lowered(path: &str) -> String {
    path.to_lowercase().replace('/', "\\")
}

/// 去掉结尾分隔符后的目录形式；"" 表示无效输入，用于避免把整盘当成子目录
pub(crate) fn normalized_dir(dir: &str) -> String {
    lowered(dir).trim_end_matches(['\\', '/']).to_string()
}

/// path 是否位于 dir 之内（不含 dir 自身）；"D:\v" 不会匹配 "D:\vids2\x.mp4"
pub(crate) fn path_under(path: &str, dir: &str) -> bool {
    let prefix = format!("{}\\", normalized_dir(dir));
    !prefix.is_empty() && prefix != "\\" && lowered(path).starts_with(&prefix)
}

/// 需要一并忘掉的扫描根：与被移除目录相同，或位于其内部。
/// 上层根目录保持原样——它覆盖的范围不止这里，不该被顺带清掉。
pub(crate) fn roots_to_forget(roots: &[String], dir: &str) -> Vec<String> {
    let target = normalized_dir(dir);
    roots
        .iter()
        .filter(|r| normalized_dir(r) == target || path_under(r, dir))
        .cloned()
        .collect()
}

/// 从库中移除某个目录：忘掉指向它的扫描根，删除该目录下的条目与封面缓存。
/// 只动应用内的记录，磁盘文件一律不删——要删文件请用条目上的删除按钮。
#[tauri::command]
pub fn remove_media_directory(
    app: tauri::AppHandle,
    state: State<AppState>,
    kind: String,
    dir: String,
) -> Result<usize, String> {
    let key = roots_key(&kind)?;
    if normalized_dir(&dir).is_empty() {
        return Err("目录路径为空，未做任何修改".into());
    }
    // roots_key 已保证 kind 只会是 video / image
    let video = kind == "video";

    let removed_ids: Vec<String> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let ids: Vec<String> = if video {
            db::get_all_videos(&conn)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|v| path_under(&v.path, &dir))
                .map(|v| v.id)
                .collect()
        } else {
            db::get_all_images(&conn)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|i| path_under(&i.path, &dir))
                .map(|i| i.id)
                .collect()
        };
        // 与扫描一致：整批删除要么全部生效，要么全部回滚
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        if video {
            db::delete_videos_by_ids(&tx, &ids)
        } else {
            db::delete_images_by_ids(&tx, &ids)
        }
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        ids
    };

    for id in &removed_ids {
        clear_thumbnail_cache(&app, id);
    }

    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let raw = db::get_setting(&conn, key).map_err(|e| e.to_string())?;
        let roots: Vec<String> = raw
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default();
        let forgotten = roots_to_forget(&roots, &dir);
        if !forgotten.is_empty() {
            let kept: Vec<String> = roots
                .into_iter()
                .filter(|r| !forgotten.contains(r))
                .collect();
            let value = serde_json::to_string(&kept).map_err(|e| e.to_string())?;
            db::set_setting(&conn, key, &value).map_err(|e| e.to_string())?;
        }
    }

    Ok(removed_ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_under_ignores_case_and_separators() {
        assert!(path_under(r"D:\pics\a\b.jpg", r"D:\pics"));
        assert!(path_under(r"D:/pics/a/b.jpg", r"d:\pics\"));
        assert!(path_under(r"D:\PICS\b.jpg", r"d:\pics"));
        // 目录本身不算"在其之内"
        assert!(!path_under(r"D:\pics", r"D:\pics"));
        assert!(!path_under(r"D:\vids2\a.mp4", r"D:\v"));
        assert!(!path_under(r"C:\other\a.jpg", r"D:\pics"));
        // 空目录不该把整盘当成它的子目录
        assert!(!path_under(r"D:\pics\a.jpg", ""));
    }

    #[test]
    fn test_roots_to_forget_keeps_parent_and_unrelated_roots() {
        let roots = vec![
            r"D:\pics".to_string(),
            r"D:\pics\2024".to_string(),
            r"D:\videos".to_string(),
        ];
        assert_eq!(
            roots_to_forget(&roots, r"d:\pics\2024"),
            vec![r"D:\pics\2024".to_string()]
        );
        // 移除父目录时，指向自身和内部子目录的根一起忘掉，无关根保留
        assert_eq!(
            roots_to_forget(&roots, r"D:\pics"),
            vec![r"D:\pics".to_string(), r"D:\pics\2024".to_string()]
        );
        assert!(roots_to_forget(&roots, r"E:\nowhere").is_empty());
    }

    #[test]
    fn test_remove_directory_drops_records_but_not_files_of_other_roots() {
        let conn = db::setup_test_db();
        db::insert_image(&conn, &db::sample_image("i1", r"D:\pics\a.png")).unwrap();
        db::insert_image(&conn, &db::sample_image("i2", r"D:\pics\sub\b.png")).unwrap();
        db::insert_image(&conn, &db::sample_image("i3", r"D:\videos\c.png")).unwrap();

        let ids: Vec<String> = db::get_all_images(&conn)
            .unwrap()
            .into_iter()
            .filter(|i| path_under(&i.path, r"D:\pics"))
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, vec!["i1".to_string(), "i2".to_string()]);
        db::delete_images_by_ids(&conn, &ids).unwrap();

        let left: Vec<String> = db::get_all_images(&conn)
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(left, vec!["i3".to_string()]);
    }
}
