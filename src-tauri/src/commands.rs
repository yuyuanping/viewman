use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::Manager;

mod image_anim;
mod image_detect;
mod images;
mod players;
mod progress;
mod removal;
mod scan;
mod settings;
mod thumbnails;
mod video_detect;
mod videos;

// 对 lib.rs 的 invoke_handler 保持平铺的命令路径（commands::scan_directory 等）
pub use image_anim::*;
pub use image_detect::*;
pub use images::*;
pub use players::*;
pub use progress::*;
pub use removal::*;
pub use scan::*;
pub use settings::*;
pub use thumbnails::*;
pub use video_detect::*;
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
    /// 正在跑封面批次的库（按进度事件名占位）：挡住同一库的两个批次并发——
    /// 启动自动续跑和用户手动点击撞上、StrictMode 双挂载各调一次续跑，都靠它兜住
    pub thumb_running: Mutex<std::collections::HashSet<&'static str>>,
}

/// 命令返回的错误统一是 String（前端拿到就直接显示），所以满地 `.map_err(|e| e.to_string())`。
/// 这个 trait 给它一个短名，少写 20 个字符也少一个闭包。
pub trait MapErrStr<T> {
    /// `x.map_err(|e| e.to_string())` → `x.map_err_str()`
    fn map_err_str(self) -> Result<T, String>;
}

impl<T, E: std::fmt::Display> MapErrStr<T> for Result<T, E> {
    fn map_err_str(self) -> Result<T, String> {
        self.map_err(|e| e.to_string())
    }
}

/// 检测结果的落盘缓存（都放 app_data 下）：三趟检测——重复图、相似图、重复视频——
/// 都是"点开一次要等几分钟"的活，关掉应用不该等于把上一趟的结果也丢了。
pub(crate) const SIMILAR_CACHE: &str = "similar-cache.json";
pub(crate) const DUPLICATE_CACHE: &str = "duplicate-cache.json";
pub(crate) const VIDEO_DUPLICATE_CACHE: &str = "video-duplicate-cache.json";

pub(crate) fn cache_path(app: &tauri::AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err_str()?;
    Ok(dir.join(name))
}

/// 先写临时文件再改名：中途关掉应用也不会留下半份 JSON（读回来要么完整要么没有）。
/// 收路径而不是 AppHandle，单测就能拿临时目录直接验这套读写。
pub(crate) fn write_cache_file<T: serde::Serialize>(path: &Path, value: &T) {
    let Ok(json) = serde_json::to_string(value) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// 读不回来（没这份 / 只写了一半 / 老版本的字段对不上）一律当"没有缓存"，界面自然会重跑一趟
pub(crate) fn read_cache_file<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

/// 写一份检测结果。调用点都在 spawn_blocking 里（本来就是阻塞上下文），直接写盘即可
pub(crate) fn write_cache<T: serde::Serialize>(app: &tauri::AppHandle, name: &str, value: &T) {
    let Ok(path) = cache_path(app, name) else { return };
    write_cache_file(&path, value);
}

/// 读一份检测结果。相似缓存能到几 MB（每张成组图的 id + 指纹都在里面），
/// 所以读盘和解析都挪到阻塞线程池，别占着 async runtime 的线程。
pub(crate) async fn read_cache<T: serde::de::DeserializeOwned + Send + 'static>(
    app: &tauri::AppHandle,
    name: &str,
) -> Result<Option<T>, String> {
    let path = cache_path(app, name)?;
    tauri::async_runtime::spawn_blocking(move || read_cache_file::<T>(&path))
        .await
        .map_err_str()
}

/// 删一份检测结果：面板上按 ✕ 是"我不认这份结果"，缓存得跟着作废，不然下次打开又给恢复回来
pub(crate) fn remove_cache(app: &tauri::AppHandle, name: &str) {
    if let Ok(path) = cache_path(app, name) {
        let _ = std::fs::remove_file(path);
    }
}

// ── 批量写库的共享设施 ──────────────────────────────────────────────
// images.rs 与 videos.rs 的检测命令都靠"攒一批 → 抢锁 → 写库 → 清批次"滚动落盘，
// 抢不到锁或写失败就原样留着等下一轮。两边以前各写一套，现在统一在此。

/// 抢并行累加器的锁。这些 Mutex 里装的就是个普通集合，毒化只说明别处 panic 过、
/// 内容本身仍然可用；而抢锁时 panic 会顺着 scope 的 join 把整个检测命令带崩——
/// 检测跑几分钟白跑，界面只看到一条"命令失败"。
pub(crate) fn lock_ignoring_poison<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 抢锁落库：抢不到（启动扫描正占着同一把锁）或写失败就原样留着这批，返回有没有写进去。
/// 以前是"抢不到就 clear"，等于这一截 ffmpeg 白跑，下次还得从头算。
pub(crate) fn flush<T>(
    app: &tauri::AppHandle,
    batch: &mut Vec<T>,
    save: impl Fn(&rusqlite::Connection, &[T]) -> rusqlite::Result<()>,
) -> bool {
    if batch.is_empty() {
        return true;
    }
    match app.state::<AppState>().db.lock() {
        Ok(conn) if save(&conn, batch).is_ok() => {
            batch.clear();
            true
        }
        _ => false,
    }
}

/// 收尾时给没写进去的批次几次重试：扫描的写库是一阵一阵的，等得起
pub(crate) fn flush_with_retry<T>(
    app: &tauri::AppHandle,
    batch: &mut Vec<T>,
    save: impl Fn(&rusqlite::Connection, &[T]) -> rusqlite::Result<()>,
) {
    for _ in 0..10 {
        if flush(app, batch, &save) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// 并行线程数：按 CPU 核数取，封顶 14（再高也吃不住逐条 spawn ffmpeg 的开销），不超任务总数
pub(crate) fn workers(total: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 14)
        .min(total.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落一份缓存的形状在临时目录里验：不碰真实 app_data，并行跑也不会互撞
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("viewman-{name}-{}.json", uuid::Uuid::new_v4()))
    }

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        library_count: usize,
        groups: Vec<Vec<String>>,
    }

    #[test]
    fn test_cache_file_round_trips_and_leaves_no_temp_behind() {
        let path = scratch("round-trip");
        let payload = Payload { library_count: 7, groups: vec![vec!["a".to_string(), "b".to_string()]] };

        write_cache_file(&path, &payload);

        assert_eq!(read_cache_file::<Payload>(&path), Some(payload));
        // 临时文件必须已经被改名走：留一份在盘上，下次写失败时读到的就是过期内容
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_cache_file_replaces_the_previous_result() {
        let path = scratch("replace");
        write_cache_file(&path, &Payload { library_count: 1, groups: Vec::new() });

        write_cache_file(&path, &Payload { library_count: 9, groups: vec![vec!["z".to_string()]] });

        let back = read_cache_file::<Payload>(&path).expect("第二次写的结果该读得回来");
        assert_eq!(back.library_count, 9);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_read_cache_file_treats_missing_and_broken_json_as_no_cache() {
        // 没跑过检测：读不到就是 null，界面按"没结果"处理
        assert_eq!(read_cache_file::<Payload>(&scratch("missing")), None);

        // 半份 JSON（写途中被动过）不能当缓存用，否则界面会拿到一个空结果
        let broken = scratch("broken");
        std::fs::write(&broken, "{ \"libraryCount\": 3, \"grou").unwrap();
        assert_eq!(read_cache_file::<Payload>(&broken), None);

        let empty = scratch("empty");
        std::fs::write(&empty, "").unwrap();
        assert_eq!(read_cache_file::<Payload>(&empty), None);

        let _ = std::fs::remove_file(&broken);
        let _ = std::fs::remove_file(&empty);
    }

    #[test]
    fn test_similar_cache_json_keeps_the_keys_the_frontend_reads() {
        // 前端按 libraryCount / threshold / result 读，指纹按 lo/hi 算距离——
        // 改字段名就是改跨语言接口，这个测试是那道闸
        let cache = SimilarCache {
            threshold: 10,
            library_count: 3,
            result: SimilarResult {
                groups: vec![vec!["a".to_string(), "b".to_string()]],
                far: Vec::new(),
                hashes: vec![SimilarHit { id: "a".to_string(), lo: 1, hi: 2 }],
            },
        };

        let json = serde_json::to_string(&cache).unwrap();

        assert!(json.contains("\"libraryCount\":3"), "{json}");
        assert!(json.contains("\"threshold\":10"), "{json}");
        assert!(json.contains("\"groups\":[[\"a\",\"b\"]]"), "{json}");
        assert!(json.contains("\"hashes\":[{\"id\":\"a\",\"lo\":1,\"hi\":2}]"), "{json}");
    }

    #[test]
    fn test_video_duplicate_cache_json_keeps_the_keys_the_frontend_reads() {
        let cache = VideoDuplicateCache {
            library_count: 2,
            groups: vec![vec!["v1".to_string(), "v2".to_string()]],
        };

        let json = serde_json::to_string(&cache).unwrap();

        assert!(json.contains("\"libraryCount\":2"), "{json}");
        assert!(json.contains("\"groups\":[[\"v1\",\"v2\"]]"), "{json}");
    }
}
