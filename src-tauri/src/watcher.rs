//! 扫描根目录监视：文件变化防抖聚合后自动增量扫描，免手动。
//!
//! 设计：
//! - 两套库（视频/图片）各自的根共用一个 watcher 线程，按根目录分派到对应类型
//! - 事件不立即扫描：3 秒防抖窗口聚合，把窗口内命中的 (根, 类型) 收集起来一次处理，
//!   避免下载器边写边触发几十轮扫描
//! - 只监听创建/修改/删除/重命名（纯元数据变化忽略），递归整个根
//! - 扫描失败（磁盘拔了等）静默跳过：下次事件再来时再试，不产生错误弹窗

use std::collections::HashSet;
use std::path::{Path, PathBuf};
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
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_))
    )
}

/// 抢锁时忽略毒化：这几个锁里装的都是普通集合/连接，毒化只说明别处 panic 过，
/// 内容本身还能用。而在这条后台线程上 panic 的代价是整个自动扫描静默失效——
/// 那比拿到一份刚被别处打断的数据糟得多。
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 启动后台监视线程。watcher 建不起来只打日志——不影响手动扫描。
pub(crate) fn spawn(app: tauri::AppHandle) {
    let roots = read_roots(&app);

    // notify 6 的 recommended_watcher 只接受无界 mpsc::Sender，有界背压做在消费侧：
    // 防抖循环内 try_recv 一次收走突发堆积，pending 按 (根, 类型) 去重，队列本身不跨窗口堆积
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
        let mut w = lock(&watched);
        for (kind, root) in &roots {
            if watcher.watch(root, RecursiveMode::Recursive).is_ok() {
                w.insert((root.clone(), kind));
            }
        }
    }

    // 线程起不来也只是没有自动扫描，别把启动流程一起带走
    if let Err(e) = std::thread::Builder::new()
        .name("dir-watcher".into())
        .spawn(move || event_loop(app, rx, watched, watcher))
    {
        eprintln!("目录监视线程起不来（不影响手动扫描）：{e}");
    }
}

/// 读出全部扫描根：(类型, 根) 列表
fn read_roots(app: &tauri::AppHandle) -> Vec<(&'static str, PathBuf)> {
    let state = app.state::<AppState>();
    let conn = lock(&state.db);
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

/// 事件主循环：防抖聚合 → 命中的根各跑一次增量扫描 → 刷新 watch 列表。
/// 失败退避：同一根连续失败时按 30s×次数（封顶 10min）跳过，避免拔盘后每 3s 全量 walk 一次空转。
fn event_loop(
    app: tauri::AppHandle,
    rx: Receiver<notify::Result<notify::Event>>,
    watched: Mutex<HashSet<(PathBuf, &'static str)>>,
    mut watcher: notify::RecommendedWatcher,
) {
    let mut pending: HashSet<PendingScan> = HashSet::new();
    let mut backoff: std::collections::HashMap<(PathBuf, &'static str), (u32, std::time::Instant)> =
        std::collections::HashMap::new();
    loop {
        // 窗口起点：等到第一条事件；无事件时阻塞
        let first = match rx.recv() {
            Ok(ev) => ev,
            Err(_) => return,
        };
        if let Some(p) = classify(&first, &watched) {
            pending.insert(p);
        }
        // 静默期满才放行；期间新事件继续入桶。通道有界时这里 drain 掉突发堆积，
        // 只保留 (根, 类型) 去重结果，队列本身不会胀
        loop {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(ev) => {
                    if let Some(p) = classify(&ev, &watched) {
                        pending.insert(p);
                    }
                    // 突发洪峰时把已排队的也一次收走，避免串行 3s 窗口拖长尾
                    while let Ok(ev) = rx.try_recv() {
                        if let Some(p) = classify(&ev, &watched) {
                            pending.insert(p);
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        // 串行扫：本线程就是后台线程，避免并发写库
        for p in pending.drain() {
            let key = (p.root.clone(), p.kind);
            if let Some((fails, since)) = backoff.get(&key) {
                let wait = std::time::Duration::from_secs((*fails as u64 * 30).min(600));
                if since.elapsed() < wait {
                    continue;
                }
            }
            if run_scan(&app, p.kind, &p.root) {
                backoff.remove(&key);
            } else {
                let fails = backoff.get(&key).map(|(f, _)| *f).unwrap_or(0) + 1;
                backoff.insert(key, (fails, std::time::Instant::now()));
            }
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
    let w = lock(watched);
    for (root, kind) in w.iter() {
        for path in &ev.paths {
            if path.starts_with(root) {
                return Some(PendingScan {
                    kind,
                    root: root.clone(),
                });
            }
        }
    }
    None
}

/// 对一个根跑增量扫描，把本轮的增删原样广播出去（前端就地合并，不再整库重拉）。
/// 失败静默（磁盘可能被拔走），下轮事件再试；失败时不广播，库本来就没改动。
/// 返回是否成功，供事件循环做失败退避。
fn run_scan(app: &tauri::AppHandle, kind: &'static str, root: &PathBuf) -> bool {
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
        false
    } else {
        true
    }
}

/// 一条数据都没变时不打扰前端：合并空增量也要重排整表
fn emit_outcome<T: serde::Serialize + Clone>(
    app: &tauri::AppHandle,
    event: &str,
    outcome: ScanOutcome<T>,
) {
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
    let mut w = lock(watched);
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

/// bot.db 变化监视：QQ 面板下载列表的实时推送源。
/// bot 每次落库（明细行预登记/进度/终态）都写 bot.db(-wal)，这里对数据目录做
/// 非递归监视，防抖后发一次 bot-dl-changed，前端拉一次轻量快照，叠在 10s
/// 全量轮询之上——进程/端口状态仍靠轮询，这里只管下载列表。
/// 建不起来（notify 不可用/目录缺失）就静默不推，10s 轮询兜底。
const BOT_DB_DEBOUNCE: Duration = Duration::from_millis(500);

pub(crate) fn spawn_bot_db(app: tauri::AppHandle) {
    let db_path = {
        let state = app.state::<AppState>();
        let settings = match crate::commands::get_settings(&state) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("bot.db 监视未启用（读设置失败，退回 10s 轮询）：{e}");
                return;
            }
        };
        crate::commands::bot_db_path(&settings.root)
    };
    let data_dir = db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("bot.db 监视不可用（退回 10s 轮询）：{e}");
            return;
        }
    };
    if let Err(e) = watcher.watch(&data_dir, RecursiveMode::NonRecursive) {
        eprintln!("bot.db 目录监视建不起来（退回 10s 轮询）：{e}");
        return;
    }
    if let Err(e) = std::thread::Builder::new()
        .name("botdb-watcher".into())
        .spawn(move || bot_db_loop(app, rx, db_path))
    {
        eprintln!("bot.db 监视线程起不来（退回 10s 轮询）：{e}");
    }
}

/// 命中 bot.db*（含 -wal/-journal）才防抖发事件；config/log 等其余文件忽略。
fn bot_db_loop(
    app: tauri::AppHandle,
    rx: Receiver<notify::Result<notify::Event>>,
    db_path: PathBuf,
) {
    loop {
        let first = match rx.recv() {
            Ok(ev) => ev,
            Err(_) => return, // watcher 已销毁，应用在退出
        };
        if !is_bot_db(&first, &db_path) {
            continue;
        }
        // 防抖：窗口内的连续写（WAL 刷盘常成串）吸收成一次事件
        while rx.recv_timeout(BOT_DB_DEBOUNCE).is_ok() {}
        if app.emit("bot-dl-changed", ()).is_err() {
            return; // 应用已退出
        }
    }
}

fn is_bot_db(ev: &notify::Result<notify::Event>, db_path: &Path) -> bool {
    match ev {
        Ok(event) => event.paths.iter().any(|p| {
            p == db_path
                || p
                    .file_name()
                    .map(|n| n.to_string_lossy().starts_with("bot.db"))
                    .unwrap_or(false)
        }),
        Err(_) => false,
    }
}
