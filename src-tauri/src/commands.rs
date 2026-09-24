use std::path::Path;
use std::sync::Mutex;

mod images;
mod players;
mod progress;
mod removal;
mod scan;
mod settings;
mod thumbnails;
mod videos;

// 对 lib.rs 的 invoke_handler 保持平铺的命令路径（commands::scan_directory 等）
pub use images::*;
pub use players::*;
pub use progress::*;
pub use removal::*;
pub use scan::*;
pub use settings::*;
pub use thumbnails::*;
pub use videos::*;

/// 把 (id, 路径) 一批送进回收站，返回没能删掉的那些。视频库与图片库共用。
/// 整批只开一次 shell 事务（`trash::delete_all` 内部就是一个 IFileOperation），
/// 逐张送的话每张都要一次回收站注册 + 一次刷新。
/// 磁盘上已经不存在的一律视作已删除——库记录必须能清掉，否则残留显示。
pub(crate) fn undeleted_targets(targets: &[(String, String)]) -> Vec<(String, String)> {
    let present: Vec<&(String, String)> = targets
        .iter()
        .filter(|(_, path)| Path::new(path).exists())
        .collect();
    if present.is_empty() {
        return Vec::new();
    }

    let paths: Vec<&str> = present.iter().map(|(_, path)| path.as_str()).collect();
    if trash::delete_all(&paths).is_ok() {
        // 整批成功：仍在磁盘上的才算没删掉
        return present
            .into_iter()
            .filter(|(_, path)| Path::new(path).exists())
            .cloned()
            .collect();
    }

    // 一个文件被占用就足以让整批报错 → 退回逐张，只留下真正删不掉的那些
    let mut survivors = Vec::new();
    for (id, path) in present {
        if trash::delete(path).is_err() && Path::new(path).exists() {
            survivors.push((id.clone(), path.clone()));
        }
    }
    survivors
}

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
}
