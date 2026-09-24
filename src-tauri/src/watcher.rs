//! 扫描根目录监视：文件变化防抖聚合后自动增量扫描，免手动。
//!
//! 设计：
//! - 两套库（视频/图片）各自的根共用一个 watcher 线程，按根目录分派到对应类型
//! - 事件不立即扫描：3 秒防抖窗口聚合，把窗口内命中的 (根, 类型) 收集起来一次处理，
//!   避免下载器边写边触发几十轮扫描
//! - 只监听创建/修改/删除/重命名（纯元数据变化忽略），递归整个根
//! - 扫描失败（磁盘拔了等）静默跳过：下次事件再来时再试，不产生错误弹窗

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Mutex;
use std::time::Duration;

use notify::event::{EventKind, ModifyKind};
use notify::{RecursiveMode, Watcher};
use tauri::{Emitter, Manager};

// scan_directory / scan_image_directory 经 commands.rs 平铺 re-export，直接可用
use crate::commands::{scan_directory, scan_image_directory, AppState};
use crate::models::ScanOutcome;

/// 防抖窗口：事件静默这么久后才真正触发扫描
const DEBOUNCE: Duration = Duration::from_secs(3);

/// 扫描根的 settings 键（与 commands::settings 常量保持一致；那边是 pub(crate) 常量，
/// 但模块私有路径不可达，这里按字面值对齐——改键名时两处同步）
const VIDEO_ROOTS_KEY: &str = "scan_roots";
const IMAGE_ROOTS_KEY: &str = "image_scan_roots";

/// 一条待扫描任务：哪个类型的哪个根目录发生了变化
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct PendingScan {
    kind: &'static str, // "video" | "image"
    root: PathBuf,
}

/// 变化事件是否值得触发扫描（忽略纯元数据/属性变化）
fn is_content_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Create(_) => true,
        EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_)) => true,
        _ => false,
    }
}

/// 启动后台监视线程。watcher 建不起来只打日志——不影响手动扫描。
pub(crate) fn spawn(app: tauri::AppHandle) {
    let roots = read_roots(&app);

    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("目录监视不可用（不影响手动扫描）：{e}");
            return;
        }
    };

    let watched: Mutex<HashSet<(PathBuf, &'static str)>> = Mutex::new(HashSet::new());
    {
        let mut w = watched.lock().unwrap();
        for (kind, root) in &roots {
            if watcher.watch(root, RecursiveMode::Recursive).is_ok() {
                w.insert((root.clone(), kind));
            }
        }
    }

    std::thread::Builder::new()
        .name("dir-watcher".into())
        .spawn(move || event_loop(app, rx, watched, watcher))
        .expect("failed to spawn dir watcher thread");
}

/// 读出全部扫描根：(类型, 根) 列表
fn read_roots(app: &tauri::AppHandle) -> Vec<(&'static str, PathBuf)> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().unwrap();
    let mut out = Vec::new();
    for (key, kind) in [(VIDEO_ROOTS_KEY, "video"), (IMAGE_ROOTS_KEY, "image")] {
        if let Ok(Some(raw)) = crate::db::get_setting(&conn, key) {
            if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
                for dir in list {
                    out.push((kind, PathBuf::from(dir)));
                }
            }
        }
    }
    out
}

/// 事件主循环：防抖聚合 → 命中的根各跑一次增量扫描 → 刷新 watch 列表
fn event_loop(
    app: tauri::AppHandle,
    rx: Receiver<notify::Result<notify::Event>>,
    watched: Mutex<HashSet<(PathBuf, &'static str)>>,
    mut watcher: notify::RecommendedWatcher,
) {
    let mut pending: HashSet<PendingScan> = HashSet::new();
    loop {
        // 窗口起点：等到第一条事件；无事件时阻塞
        let first = match rx.recv() {
            Ok(ev) => ev,
            Err(_) => return,
        };
        if let Some(p) = classify(&first, &watched) {
            pending.insert(p);
        }
        // 静默期满才放行；期间新事件继续入桶
        loop {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(ev) => {
                    if let Some(p) = classify(&ev, &watched) {
                        pending.insert(p);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        // 串行扫：本线程就是后台线程，避免并发写库
        for p in pending.drain() {
            run_scan(&app, p.kind, &p.root);
        }
        // 重读根表：用户新加/移除的扫描根动态跟进
        refresh_watch_list(&app, &watched, &mut watcher);
    }
}

/// 事件 → 属于哪个 (根, 类型)。文件路径落在已 watch 的根下才算
fn classify(
    ev: &notify::Result<notify::Event>,
    watched: &Mutex<HashSet<(PathBuf, &'static str)>>,
) -> Option<PendingScan> {
    let ev = ev.as_ref().ok()?;
    if !is_content_change(&ev.kind) {
        return None;
    }
    let w = watched.lock().unwrap();
    for (root, kind) in w.iter() {
        for path in &ev.paths {
            if path.starts_with(root) {
                return Some(PendingScan { kind, root: root.clone() });
            }
        }
    }
    None
}

/// 对一个根跑增量扫描，把本轮的增删原样广播出去（前端就地合并，不再整库重拉）。
/// 失败静默（磁盘可能被拔走），下轮事件再试；失败时不广播，库本来就没改动。
fn run_scan(app: &tauri::AppHandle, kind: &'static str, root: &PathBuf) {
    let dir = root.to_string_lossy().to_string();
    let state = app.state::<AppState>();
    let result = match kind {
        "video" => tauri::async_runtime::block_on(scan_directory(app.clone(), state, dir))
            .map(|outcome| emit_outcome(app, "videos-changed", outcome)),
        "image" => tauri::async_runtime::block_on(scan_image_directory(app.clone(), state, dir))
            .map(|outcome| emit_outcome(app, "images-changed", outcome)),
        _ => Ok(()),
    };
    if let Err(e) = result {
        eprintln!("自动重扫失败（{kind} {root:?}）：{e}");
    }
}

/// 一条数据都没变时不打扰前端：合并空增量也要重排整表
fn emit_outcome<T: serde::Serialize + Clone>(app: &tauri::AppHandle, event: &str, outcome: ScanOutcome<T>) {
    if outcome.items.is_empty() && outcome.removed_ids.is_empty() {
        return;
    }
    let _ = app.emit(event, outcome);
}

/// 根表重读：新增的根挂上 watch；已移除的根取消 watch
fn refresh_watch_list(
    app: &tauri::AppHandle,
    watched: &Mutex<HashSet<(PathBuf, &'static str)>>,
    watcher: &mut notify::RecommendedWatcher,
) {
    let current: HashSet<(PathBuf, &'static str)> =
        read_roots(app).into_iter().map(|(k, r)| (r, k)).collect();
    let mut w = watched.lock().unwrap();
    // 新增：watch 成功才入表（difference 结果先收走，别在借用 w 的循环里改 w）
    let added: Vec<(PathBuf, &'static str)> = current.difference(&w).cloned().collect();
    for (root, kind) in added {
        if watcher.watch(&root, RecursiveMode::Recursive).is_ok() {
            w.insert((root, kind));
        }
    }
    // 移除：不再在根表里的取消 watch 并出表
    let removed: Vec<(PathBuf, &'static str)> = w.difference(&current).cloned().collect();
    for (root, kind) in removed {
        let _ = watcher.unwatch(&root);
        w.remove(&(root, kind));
    }
}
