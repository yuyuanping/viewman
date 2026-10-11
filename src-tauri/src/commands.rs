use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::db;
use rusqlite::Connection;
use tauri::Manager;

mod image_anim;
mod image_detect;
mod images;
mod maintenance;
mod napcat;
mod players;
mod progress;
mod qqbot;
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
pub use maintenance::*;
pub use napcat::*;
pub use players::*;
pub use progress::*;
pub use qqbot::*;
pub use removal::*;
pub use scan::*;
pub use settings::*;
pub use thumbnails::*;
pub use video_detect::*;
pub use videos::*;

/// 缓存根目录（thumbnails/transcoded 的父目录）：settings 表 cache_dir 说了算，
/// 空缺退 app_data。给 C 盘紧张/想把缓存放别的盘的机器一个出口
pub(crate) fn cache_base(app: &tauri::AppHandle) -> std::path::PathBuf {
    let fallback = || {
        app.path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
    };
    let state = app.state::<AppState>();
    let Ok(conn) = state.db.lock() else {
        return fallback();
    };
    let configured = db::get_setting(&conn, "cache_dir")
        .ok()
        .flatten()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    drop(conn);
    let Some(dir) = configured else {
        return fallback();
    };
    let p = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&p);
    p
}

/// settings 表里的正整数配置（video_scan_workers / image_scan_workers 等）
pub(crate) fn setting_usize(app: &tauri::AppHandle, key: &str, default: usize) -> usize {
    let state = app.state::<AppState>();
    let Ok(conn) = state.db.lock() else {
        return default;
    };
    db::get_setting(&conn, key)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// GUI 进程里 spawn 控制台程序（powershell/netstat/taskkill/cmd）默认会弹黑框。
/// qqbot/napcat 的状态轮询每 5-10 秒就要 spawn 一次，必须用这个把窗口压掉
/// （CREATE_NO_WINDOW），否则打包出来的应用会一直闪黑窗口。
pub(crate) fn silent_command(program: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new(program);
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd
    }
    #[cfg(not(windows))]
    std::process::Command::new(program)
}

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

    // 一个文件被占用就足以让整批报错 → 退回逐张，只留下真正删不掉的那些。
    // 逐张带 6 次退避重试（200/400/800…ms，约 3s）：自家 ffmpeg 解码（检测/封面正好
    // 在读这张）、杀软扫尾这类秒级瞬时锁等一会儿就过去了，直接报"被占用"太冤；
    // 资源管理器预览窗/照片应用这类常驻锁才留到最后报给用户。
    const BACKOFF_MS: &[u64] = &[0, 200, 400, 800, 800, 800];
    let mut survivors = Vec::new();
    for (id, path) in present {
        let mut ok = false;
        for wait in BACKOFF_MS {
            if *wait > 0 {
                std::thread::sleep(std::time::Duration::from_millis(*wait));
            }
            if trash::delete(path).is_ok() || !Path::new(path).exists() {
                ok = true;
                break;
            }
        }
        if !ok && Path::new(path).exists() {
            survivors.push((id.clone(), path.clone()));
        }
    }
    survivors
}

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    /// 库文件路径：只读查询走 open_read_conn 新开连接（WAL 并发读），不再跟写抢全局锁
    pub db_path: String,
    /// 长任务取消标记：key 为任务名（scan-video/scan-image/hevc/thumb-*），前端取消按钮置 true，
    /// 后台循环每批检查一次。Mutex<HashMap> 够用，检查频率低。
    pub cancel: Mutex<std::collections::HashMap<String, bool>>,
    /// 正在跑封面批次的库（按进度事件名占位）：挡住同一库的两个批次并发——
    /// 启动自动续跑和用户手动点击撞上、StrictMode 双挂载各调一次续跑，都靠它兜住
    pub thumb_running: Mutex<std::collections::HashSet<&'static str>>,
}

/// 是否疑似库损坏类错误（malformed/corrupt）：删/写在这种错上值得原地重建 FTS 后重试一次
pub(crate) fn is_corruption_error(e: &str) -> bool {
    let lower = e.to_lowercase();
    lower.contains("malformed") || lower.contains("corrupt")
}

/// 删库记录（带自愈）：首次失败若疑似损坏，先无损重建 FTS 再重试一次；
/// 仍失败则附上即时自检结果，方便定位是 FTS 还是主表。
/// 调用时事务已由内部管理；文件先行进了回收站，重试时"磁盘已无文件"按已删除计。
pub(crate) fn delete_records_with_heal(
    conn: &rusqlite::Connection,
    ids: &[String],
    kind: &str,
    delete: impl Fn(&rusqlite::Connection, &[String]) -> rusqlite::Result<()>,
) -> Result<(), String> {
    let attempt = || -> Result<(), rusqlite::Error> {
        let tx = conn.unchecked_transaction()?;
        delete(&tx, ids)?;
        tx.commit()?;
        Ok(())
    };
    match attempt() {
        Ok(()) => Ok(()),
        Err(e) if is_corruption_error(&e.to_string()) => {
            if let Err(re) = crate::db::rebuild_fts(conn) {
                return Err(format!(
                    "删除{}库记录失败（文件已移入回收站）：{}；自动重建索引也失败：{}。请用启动自检的「重新修复」。",
                    kind, e, re
                ));
            }
            attempt().map_err(|e2| {
                let check = crate::db::integrity_check(conn)
                    .unwrap_or_else(|c| format!("自检失败：{}", c));
                format!(
                    "删除{}库记录失败（文件已移入回收站）：{}；重建索引后重试仍失败，自检：{}。请从备份恢复库文件。",
                    kind, e2, check
                )
            })
        }
        Err(e) => Err(format!(
            "删除{}库记录失败（文件已移入回收站，重试一次即可清理记录）：{}",
            kind, e
        )),
    }
}

/// 长任务是否已被取消（默认 false）
pub(crate) fn is_cancelled(app: &tauri::AppHandle, task: &str) -> bool {
    use tauri::Manager;
    app.state::<AppState>()
        .cancel
        .lock()
        .map(|m| m.get(task).copied().unwrap_or(false))
        .unwrap_or(false)
}

/// 发起/清除取消标记
#[tauri::command]
pub fn cancel_task(app: tauri::AppHandle, task: String) -> Result<(), String> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    state
        .cancel
        .lock()
        .map(|mut m| {
            m.insert(task, true);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn clear_cancel(app: &tauri::AppHandle, task: &str) {
    use tauri::Manager;
    if let Ok(mut m) = app.state::<AppState>().cancel.lock() {
        m.remove(task);
    }
}

/// 只读查询连接：失败时调用方回退全局锁（open 失败几乎只发生在库文件被删的极端情况）
pub(crate) fn read_conn_or_lock(app: &tauri::AppHandle) -> Result<rusqlite::Connection, String> {
    use tauri::Manager;
    let path = app.state::<AppState>().db_path.clone();
    crate::db::open_read_conn(&path).map_err(|e| e.to_string())
}

/// 命令返回的错误统一是 String（前端拿到就直接显示），所以满地 `.map_err(|e| e.to_string())`。
/// 这个 trait 给它一个短名，少写 20 个字符也少一个闭包。
/// 按 id 取视频路径，库里没有这条记录时报错
pub(crate) fn video_path_or(conn: &Connection, video_id: &str) -> Result<String, String> {
    db::get_video_path(conn, video_id)
        .map_err_str()
        .and_then(|p| p.ok_or_else(|| format!("Video not found: {}", video_id)))
}

pub trait MapErrStr<T> {
    /// `x.map_err(|e| e.to_string())` → `x.map_err_str()`
    fn map_err_str(self) -> Result<T, String>;
}

impl<T, E: std::fmt::Display> MapErrStr<T> for Result<T, E> {
    fn map_err_str(self) -> Result<T, String> {
        self.map_err(|e| e.to_string())
    }
}

/// 检测结果的落盘缓存（都放 app_data 下）：两趟检测——重复图、重复视频——
/// 都是"点开一次要等几分钟"的活，关掉应用不该等于把上一趟的结果也丢了。
pub(crate) const DUPLICATE_CACHE: &str = "duplicate-cache.json";
pub(crate) const VIDEO_DUPLICATE_CACHE: &str = "video-duplicate-cache.json";

pub(crate) fn cache_path(app: &tauri::AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err_str()?;
    Ok(dir.join(name))
}

/// 先写临时文件再改名：中途关掉应用也不会留下半份 JSON（读回来要么完整要么没有）。
/// 收路径而不是 AppHandle，单测就能拿临时目录直接验这套读写。
/// Windows 上 rename 目标存在即失败：先删目标再改名，失败清掉 tmp 不留过期缓存。
pub(crate) fn write_cache_file<T: serde::Serialize>(path: &Path, value: &T) {
    let Ok(json) = serde_json::to_string(value) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, path).is_err() {
        // Windows：目标已存在时 rename 失败，先删再试一次
        let _ = std::fs::remove_file(path);
        if std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// 读不回来（没这份 / 只写了一半 / 老版本的字段对不上）一律当"没有缓存"，界面自然会重跑一趟
pub(crate) fn read_cache_file<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

/// 写一份检测结果。调用点都在 spawn_blocking 里（本来就是阻塞上下文），直接写盘即可
pub(crate) fn write_cache<T: serde::Serialize>(app: &tauri::AppHandle, name: &str, value: &T) {
    let Ok(path) = cache_path(app, name) else {
        return;
    };
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
/// 用 try_lock 非阻塞：检测线程不等扫描的长事务，批次攒着等下一轮 flush_with_retry。
pub(crate) fn flush<T>(
    app: &tauri::AppHandle,
    batch: &mut Vec<T>,
    save: impl Fn(&rusqlite::Connection, &[T]) -> rusqlite::Result<()>,
) -> bool {
    if batch.is_empty() {
        return true;
    }
    match app.state::<AppState>().db.try_lock() {
        Ok(conn) if save(&conn, batch).is_ok() => {
            batch.clear();
            true
        }
        _ => false,
    }
}

/// 收尾时给没写进去的批次几次重试：扫描的写库是一阵一阵的，等得起。
/// 指数退避 100ms→200ms→400ms…封顶 800ms，共 10 次约 5s（配合 busy_timeout）。
pub(crate) fn flush_with_retry<T>(
    app: &tauri::AppHandle,
    batch: &mut Vec<T>,
    save: impl Fn(&rusqlite::Connection, &[T]) -> rusqlite::Result<()>,
) {
    let mut wait_ms = 100u64;
    for _ in 0..10 {
        if flush(app, batch, &save) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(wait_ms));
        wait_ms = (wait_ms * 2).min(800);
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
    fn test_is_corruption_error_matches_sqlite_wording() {
        assert!(is_corruption_error("database disk image is malformed"));
        assert!(is_corruption_error(
            "SQLITE_CORRUPT: database disk image malformed"
        ));
        assert!(!is_corruption_error("database is locked"));
        assert!(!is_corruption_error("目标文件已存在，拒绝覆盖"));
    }

    #[test]
    fn test_cache_file_round_trips_and_leaves_no_temp_behind() {
        let path = scratch("round-trip");
        let payload = Payload {
            library_count: 7,
            groups: vec![vec!["a".to_string(), "b".to_string()]],
        };

        write_cache_file(&path, &payload);

        assert_eq!(read_cache_file::<Payload>(&path), Some(payload));
        // 临时文件必须已经被改名走：留一份在盘上，下次写失败时读到的就是过期内容
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_cache_file_replaces_the_previous_result() {
        let path = scratch("replace");
        write_cache_file(
            &path,
            &Payload {
                library_count: 1,
                groups: Vec::new(),
            },
        );

        write_cache_file(
            &path,
            &Payload {
                library_count: 9,
                groups: vec![vec!["z".to_string()]],
            },
        );

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
