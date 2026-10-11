use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::State;

use crate::db;

use super::{silent_command, AppState, MapErrStr};

/// bot 工作目录 / Python 解释器：首次用内置默认值（bot 源码随仓库放在 qqbot\ 下），
/// 用户在面板里改过之后按 viewman.db settings 表里存的走
const BOT_ROOT_KEY: &str = "bot_root";
const BOT_PYTHON_KEY: &str = "bot_python";
const DEFAULT_ROOT: &str = "D:\\opencword\\viewman\\qqbot";
const DEFAULT_PYTHON: &str = "D:\\Python311\\python.exe";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotSettings {
    pub root: String,
    pub python: String,
}

/// messages 表按 dl 状态计数，即下载队列进度口径（dl=none 的文本行不占用下载槽）
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BotDlStats {
    pub none: i64,
    pub pending: i64,
    pub busy: i64,
    pub ok: i64,
    pub dead: i64,
}

/// events 表按处理状态计数：消息先入这张表才消费，堆积即积压
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BotEventStats {
    pub new: i64,
    pub busy: i64,
    pub done: i64,
    pub error: i64,
    pub dead: i64,
}

/// 下载队列一行：待下载 / 下载中 / 失败待重试的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotQueueItem {
    pub id: i64,
    pub kind: String,
    pub label: Option<String>,
    pub size: Option<i64>,
    pub tries: i64,
    /// Unix 秒：下次重试时间（busy 行正在下载，此值是上次排定的）
    pub next_try_at: Option<f64>,
    /// 最近一次失败原因，截断到 120 字
    pub err: Option<String>,
    pub dl: String,
}

/// 一次状态快照：进程 + NapCat 端口 + bot.db（只读）统计与下载队列。
/// bot.db 不存在或打不开不影响整体返回，前端拿 db_note 提示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotStatus {
    pub settings: BotSettings,
    pub bot_pid: Option<u32>,
    pub riskwatch_pid: Option<u32>,
    /// bot 是否在跑：锁住 bot.lock 即为在跑。比命令行匹配可靠——start 启动的
    /// pythonw 常常读不到 CommandLine，那种情况 bot_pid 会是 None 但这里仍为 true
    pub bot_running: bool,
    /// NapCat 的 WS(9501)/HTTP(9502) 端口监听 = QQ 协议层活着
    pub ws_up: bool,
    pub api_up: bool,
    pub dl: BotDlStats,
    pub events: BotEventStats,
    pub queue: Vec<BotQueueItem>,
    pub db_note: Option<String>,
}

fn get_settings(state: &State<AppState>) -> Result<BotSettings, String> {
    let conn = state.db.lock().map_err_str()?;
    let root = db::get_setting(&conn, BOT_ROOT_KEY)
        .map_err_str()?
        .unwrap_or_else(|| DEFAULT_ROOT.to_string());
    let python = db::get_setting(&conn, BOT_PYTHON_KEY)
        .map_err_str()?
        .unwrap_or_else(|| DEFAULT_PYTHON.to_string());
    Ok(BotSettings { root, python })
}

#[tauri::command]
pub fn load_bot_settings(state: State<AppState>) -> Result<BotSettings, String> {
    get_settings(&state)
}

#[tauri::command]
pub fn save_bot_settings(
    state: State<AppState>,
    root: String,
    python: String,
) -> Result<(), String> {
    if !Path::new(&root).join("bot.py").is_file() {
        return Err(format!("目录下找不到 bot.py: {}", root));
    }
    if python.trim().is_empty() {
        return Err("Python 路径不能为空".into());
    }
    let conn = state.db.lock().map_err_str()?;
    db::set_setting(&conn, BOT_ROOT_KEY, root.trim()).map_err_str()?;
    db::set_setting(&conn, BOT_PYTHON_KEY, python.trim()).map_err_str()
}

/// bot.db 位置：bot 的 config.json 里 data_dir 说了算，缺省退 root\bot.db
/// bot 的数据目录：config.json 的 data_dir 说了算，缺省退 root
fn data_dir(root: &str) -> std::path::PathBuf {
    if let Ok(raw) = std::fs::read_to_string(Path::new(root).join("config.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(dir) = v.get("data_dir").and_then(|x| x.as_str()) {
                return Path::new(dir).to_path_buf();
            }
        }
    }
    Path::new(root).to_path_buf()
}

fn bot_db_path(root: &str) -> std::path::PathBuf {
    data_dir(root).join("bot.db")
}

#[cfg(windows)]
mod winlock {
    // msvcrt.locking 底层就是 LockFileEx。这里直接调它，用与 bot.py 完全相同的
    // 语义探测"有没有实例在跑"，不受 start 启动后命令行读不到的限制。
    //
    // 注意 LockFileEx 的锁区间由 OVERLAPPED 里的 Offset/OffsetHigh 指定，
    // lpOverlapped 必须传有效指针，不能传 NULL。这里锁第 0 字节起的 1 字节，
    // 与 bot.py 里 msvcrt.locking(fd, LK_NBLCK, 1)（append 模式下位置为 0）一致。
    #[repr(C)]
    pub struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        h_event: *mut core::ffi::c_void,
    }

    impl Overlapped {
        pub fn new() -> Self {
            Overlapped {
                internal: 0,
                internal_high: 0,
                offset: 0,
                offset_high: 0,
                h_event: std::ptr::null_mut(),
            }
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn LockFileEx(
            h: *mut core::ffi::c_void,
            flags: u32,
            reserved: u32,
            n_low: u32,
            n_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        pub fn UnlockFileEx(
            h: *mut core::ffi::c_void,
            reserved: u32,
            n_low: u32,
            n_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    pub const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x0000_0002;
    pub const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x0000_0001;
}

/// bot.py 运行时会独占锁住 bot.lock 的第 1 字节（见 bot.py async_main 的
/// msvcrt.locking）。抢锁失败 = 有实例活着。这是判断“是否运行”最可靠的信号：
/// 不依赖命令行，也不受 pythonw 空 CommandLine 的影响。探测后立即解锁，不干扰 bot。
fn bot_lock_held(data_dir: &Path) -> bool {
    #[cfg(not(windows))]
    {
        let _ = data_dir;
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let path = data_dir.join("bot.lock");
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            // 只探测锁，绝不能截断 bot.lock（里面可能有别的进程留的数据）
            .truncate(false)
            .open(&path)
        {
            Ok(f) => f,
            Err(_) => return false,
        };
        let handle = file.as_raw_handle();
        let mut ov = winlock::Overlapped::new();
        let locked = unsafe {
            winlock::LockFileEx(
                handle,
                winlock::LOCKFILE_EXCLUSIVE_LOCK | winlock::LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut ov,
            )
        } != 0;
        if locked {
            // 抢到了说明没人在跑，立刻放开，别影响真正启动的 bot
            unsafe { winlock::UnlockFileEx(handle, 0, 1, 0, &mut ov) };
        }
        !locked
    }
}

/// 按命令行匹配 python/pythonw 进程（start.bat 同款）：bot.py 与 riskwatch.py 各找 PID。
/// 别的 Python 程序不受影响；powershell 冷启动约一秒，配 10s 轮询无压力
fn find_pids() -> (Option<u32>, Option<u32>) {
    let script = "Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR Name='pythonw.exe'\" | Where-Object { $_.CommandLine -like '*bot.py*' -or $_.CommandLine -like '*riskwatch.py*' } | ForEach-Object { if ($_.CommandLine -like '*riskwatch.py*') { \"rw $($_.ProcessId)\" } else { \"bot $($_.ProcessId)\" } }";
    let out = silent_command("powershell")
        .args(["-NoProfile", "-Command", script])
        .output();
    let (mut bot, mut rw) = (None, None);
    if let Ok(o) = out {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let mut parts = line.splitn(2, ' ');
            let pid = parts.next().and_then(|p| p.parse().ok());
            match parts.next().unwrap_or("") {
                "bot" => bot = pid,
                "rw" => rw = pid,
                _ => {}
            }
        }
    }
    (bot, rw)
}

fn port_up(port: u16) -> bool {
    let out = silent_command("netstat").args(["-ano", "-p", "tcp"]).output();
    if let Ok(o) = out {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 5
                && fields[3] == "LISTENING"
                && fields[1].rsplit(':').next().and_then(|p| p.parse().ok()) == Some(port)
            {
                return true;
            }
        }
    }
    false
}

/// 从连接读统计与下载队列；拆出来是为单测可直接喂内存库
/// 下载队列一行的原始列：id, kind, name, text 前 80 字, bytes, tries, next_try, err, dl
type QueueRow = (
    i64,
    String,
    Option<String>,
    String,
    Option<i64>,
    i64,
    f64,
    Option<String>,
    String,
);

fn read_stats(
    conn: &rusqlite::Connection,
) -> Result<(BotDlStats, BotEventStats, Vec<BotQueueItem>), String> {
    let mut dl = BotDlStats::default();
    {
        let mut stmt = conn
            .prepare("SELECT dl, COUNT(*) FROM messages GROUP BY dl")
            .map_err_str()?;
        for row in stmt
            .query_map([], |r: &rusqlite::Row| -> rusqlite::Result<(String, i64)> {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err_str()?
        {
            let (k, n) = row.map_err_str()?;
            match k.as_str() {
                "pending" => dl.pending = n,
                "busy" => dl.busy = n,
                "ok" => dl.ok = n,
                "dead" => dl.dead = n,
                _ => dl.none += n,
            }
        }
    }

    let mut events = BotEventStats::default();
    {
        let mut stmt = conn
            .prepare("SELECT status, COUNT(*) FROM events GROUP BY status")
            .map_err_str()?;
        for row in stmt
            .query_map([], |r: &rusqlite::Row| -> rusqlite::Result<(String, i64)> {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err_str()?
        {
            let (k, n) = row.map_err_str()?;
            match k.as_str() {
                "new" => events.new = n,
                "busy" => events.busy = n,
                "done" => events.done = n,
                "error" => events.error = n,
                "dead" => events.dead = n,
                _ => {}
            }
        }
    }

    // 下载队列：busy 排最前，其余按下次重试时间；上限 100 行足够面板看
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, name, substr(COALESCE(text, ''), 1, 80), bytes, tries,
                    next_try, err, dl
             FROM messages WHERE dl IN ('pending','busy','dead')
             ORDER BY CASE WHEN dl = 'busy' THEN 0 ELSE 1 END, next_try
             LIMIT 100",
        )
        .map_err_str()?;
    let mut queue: Vec<BotQueueItem> = Vec::new();
    for row in stmt
        .query_map([], |r: &rusqlite::Row| -> rusqlite::Result<QueueRow> {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })
        .map_err_str()?
    {
        let (id, kind, name, text, bytes, tries, next_try, err, state) = row.map_err_str()?;
        let trimmed_text = text.trim();
        let label = name
            .filter(|n| !n.trim().is_empty())
            .or_else(|| {
                if trimmed_text.is_empty() {
                    None
                } else {
                    Some(text.clone())
                }
            })
            .or_else(|| Some(kind.clone()))
            .unwrap_or_else(|| format!("#{}", id));
        queue.push(BotQueueItem {
            id,
            kind,
            label: Some(label),
            size: bytes,
            tries,
            next_try_at: Some(next_try),
            err: err.map(|e| e.chars().take(120).collect()),
            dl: state,
        });
    }
    Ok((dl, events, queue))
}

/// 只读打开 bot.db（WAL 库，bot 运行中也在写）拿统计；读连接短轮询 5s 等锁，不自持
fn read_bot_db(db_path: &str) -> Result<(BotDlStats, BotEventStats, Vec<BotQueueItem>), String> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let conn = rusqlite::Connection::open_with_flags(db_path, flags).map_err_str()?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err_str()?;
    read_stats(&conn)
}

#[tauri::command]
pub async fn get_bot_status(app: tauri::AppHandle) -> Result<BotStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let settings = get_settings(&state)?;
        let (bot_pid, riskwatch_pid) = find_pids();
        // 锁探针为准，命令行只用来拿 PID 展示；两者不一致时以锁为准
        let bot_running = bot_lock_held(&data_dir(&settings.root));
        let ws_up = port_up(9501);
        let api_up = port_up(9502);

        let mut dl = BotDlStats::default();
        let mut events = BotEventStats::default();
        let mut queue: Vec<BotQueueItem> = Vec::new();
        let db_path = bot_db_path(&settings.root);
        let mut db_note: Option<String> = None;
        match std::fs::metadata(&db_path) {
            Err(_) => db_note = Some(format!("数据库不存在: {}", db_path.display())),
            Ok(_) => match read_bot_db(&db_path.to_string_lossy()) {
                Ok((d, e, q)) => {
                    dl = d;
                    events = e;
                    queue = q;
                }
                Err(msg) => db_note = Some(msg),
            },
        }
        Ok(BotStatus {
            settings,
            bot_pid,
            riskwatch_pid,
            bot_running,
            ws_up,
            api_up,
            dl,
            events,
            queue,
            db_note,
        })
    })
    .await
    .map_err_str()?
}

/// 启动 = wscript 无窗跑 start_hidden.vbs（参数化路径）；start.bat 自带幂等：
/// 查 NapCat 是否活着、按命令行杀旧 bot、拉起 pythonw bot.py 与 riskwatch.py
#[tauri::command]
pub fn start_bot(state: State<AppState>) -> Result<(), String> {
    let s = get_settings(&state)?;
    let vbs = Path::new(&s.root).join("start_hidden.vbs");
    let bat = Path::new(&s.root).join("start.bat");
    if !vbs.is_file() || !bat.is_file() {
        return Err(format!(
            "启动脚本缺失: {} 或 {}",
            vbs.display(),
            bat.display()
        ));
    }
    silent_command("wscript")
        .arg(&vbs)
        .arg(&bat)
        .spawn()
        .map_err(|e| format!("启动失败: {}", e))?;
    Ok(())
}

/// 只停 bot.py 与 riskwatch（按 PID 精确结束），QQ/NapCat 不动。
/// taskkill 要等进程退出，放 spawn_blocking 里跑，避免冻结 UI 主线程
#[tauri::command]
pub async fn stop_bot() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let (bot, rw) = find_pids();
        let mut msg = String::new();
        for (name, pid) in [("bot.py", bot), ("riskwatch.py", rw)] {
            match pid {
                Some(pid) => match silent_command("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .output()
                {
                    Ok(o) if o.status.success() => {
                        msg.push_str(&format!("已停止 {} (PID {})；", name, pid))
                    }
                    Ok(o) => {
                        let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                        msg.push_str(&format!("{} 结束失败: {}；", name, err))
                    }
                    Err(e) => msg.push_str(&format!("{} 结束失败: {}；", name, e)),
                },
                None => msg.push_str(&format!("{} 未在运行；", name)),
            }
        }
        Ok(msg)
    })
    .await
    .map_err_str()?
}

/// 全停 = stop.bat：bot + riskwatch + NapCat + QQ。
/// QQ 由 NapCat 以管理员拉起时普通权限杀不掉，stop.bat 已注明建议管理员运行
#[tauri::command]
pub async fn stop_all_bot(state: State<'_, AppState>) -> Result<String, String> {
    let s = get_settings(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let bat = Path::new(&s.root).join("stop.bat");
        if !bat.is_file() {
            return Err(format!("找不到 stop.bat: {}", bat.display()));
        }
        let out = silent_command("cmd")
            .args(["/c", &bat.to_string_lossy()])
            .output()
            .map_err(|e| format!("执行 stop.bat 失败: {}", e))?;
        let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let err_text = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !err_text.is_empty() {
            text.push('\n');
            text.push_str(&err_text);
        }
        Ok(text)
    })
    .await
    .map_err_str()?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events(id INTEGER PRIMARY KEY, received_at REAL, status TEXT, payload TEXT);
             CREATE TABLE messages(id INTEGER PRIMARY KEY, kind TEXT, name TEXT, text TEXT,
                bytes INTEGER, tries INTEGER, next_try REAL, err TEXT, dl TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(id, kind, name, bytes, tries, next_try, err, dl)
             VALUES (1,'image','a.jpg',4096,2,99999.0,'timeout','pending'),
                    (2,'video',NULL,0,0,0,NULL,'busy'),
                    (3,'text',NULL,0,8,9999999.0,'api 500','dead')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events(received_at, status, payload) VALUES (0,'new','{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events(received_at, status, payload) VALUES (0,'done','{}')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn stats_and_queue_shape() {
        let conn = test_db();
        let (dl, events, queue) = read_stats(&conn).unwrap();
        assert_eq!(
            (dl.pending, dl.busy, dl.dead, dl.ok, dl.none),
            (1, 1, 1, 0, 0)
        );
        assert_eq!((events.new, events.done), (1, 1));
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].dl, "busy");
        assert_eq!(queue[1].label.as_deref(), Some("a.jpg"));
        assert_eq!(queue[1].tries, 2);
        assert_eq!(queue[2].label.as_deref(), Some("text"));
        assert_eq!(queue[2].err.as_deref(), Some("api 500"));
    }

    #[test]
    fn lock_probe_tracks_an_external_holder() {
        // 复现本次要修的 bug：bot.py 被 start "" 拉起时 CommandLine 为空，
        // 只能靠 bot.lock 判定存活。这里用真实的 LockFileEx 占住第 0 字节，
        // 断言探针能看到锁，并验证探测本身不把锁留住。
        use std::os::windows::io::AsRawHandle;
        let dir = std::env::temp_dir().join("viewman_bot_lock_test");
        let _ = std::fs::create_dir_all(&dir);
        let lock = dir.join("bot.lock");
        std::fs::write(&lock, "").unwrap();

        assert!(!bot_lock_held(&dir), "无持有者时不应判定为运行");

        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock)
            .unwrap();
        let handle = f.as_raw_handle();
        let mut ov = winlock::Overlapped::new();
        unsafe {
            winlock::LockFileEx(
                handle,
                winlock::LOCKFILE_EXCLUSIVE_LOCK | winlock::LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut ov,
            )
        };
        assert!(bot_lock_held(&dir), "锁被占住时应判定为运行");

        unsafe { winlock::UnlockFileEx(handle, 0, 1, 0, &mut ov) };
        assert!(!bot_lock_held(&dir), "释放后不应再判定为运行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn db_path_prefers_config_data_dir() {
        let dir = std::env::temp_dir().join("viewman_bot_path_test");
        let _ = std::fs::create_dir_all(&dir);
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, "{\"data_dir\":\"D:/QQBot_Data\"}").unwrap();
        assert_eq!(
            bot_db_path(dir.to_string_lossy().as_ref()),
            Path::new("D:/QQBot_Data").join("bot.db")
        );
        std::fs::write(&cfg, "not json").unwrap();
        assert_eq!(
            bot_db_path(dir.to_string_lossy().as_ref()),
            dir.join("bot.db")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
