use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::State;

use crate::db;

use super::{silent_command, AppState, MapErrStr};

/// bot 工作目录 / Python 解释器：首次用默认值（bot 源码随仓库放在 qqbot\ 下，
/// 从 exe 位置向上探测；找不到再用兜底路径），用户改过之后按 settings 表存档
const BOT_ROOT_KEY: &str = "bot_root";
const BOT_PYTHON_KEY: &str = "bot_python";
const DEFAULT_PYTHON: &str = "D:\\Python311\\python.exe";

/// 从 exe 位置向上最多 4 层找含 qqbot\bot.py 的目录（开发时 exe 在
/// target\release\ 下，往上 2 层就是仓库根）；找不到返回 None，用兜底常量
fn default_bot_root() -> Option<String> {
    let mut dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for _ in 0..4 {
        if dir.join("qqbot").join("bot.py").is_file() {
            return Some(dir.join("qqbot").to_string_lossy().to_string());
        }
        if !dir.pop() {
            return None;
        }
    }
    None
}

fn default_root() -> String {
    default_bot_root().unwrap_or_else(|| "D:\\opencword\\viewman\\qqbot".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotSettings {
    pub root: String,
    pub python: String,
}

/// messages 表按 dl 状态计数，即下载队列进度口径。侧栏徽标取 pending+busy+dead
/// 作为积压数；none 是纯文本消息不占下载槽，ok 是已完成（面板已不展示明细，
/// 只留徽标用的三项）
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BotDlStats {
    pub pending: i64,
    pub busy: i64,
    pub dead: i64,
}

/// 下载列表一行：最近 50 条有下载动作的消息（含转发归档行）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotDlItem {
    pub id: i64,
    pub kind: String,
    /// 文件原始名，没有则用文本前 80 字预览
    pub label: String,
    pub size: Option<i64>,
    /// 媒体 1=重复跳过（复用现有文件）；转发行=内部跳过的重复媒体数
    pub dedup: i64,
    pub err: Option<String>,
    pub dl: String,
}

/// 一次状态快照：进程 + NapCat 端口 + bot.db（只读）下载积压计数与最近下载列表。
/// bot.db 不存在或打不开不影响整体返回，前端拿 db_note 提示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotStatus {
    pub settings: BotSettings,
    pub bot_pid: Option<u32>,
    pub riskwatch_pid: Option<u32>,
    /// bot 是否在跑：锁住 bot.lock 即为在跑。比命令行匹配可靠——start 启动的
    /// pythonw 常常读不到 CommandLine，那种情况 bot_pid 会是 None 但这里仍为 true
    pub bot_running: bool,
    /// NapCat 的 WS/HTTP 端口监听（端口从 bot config.json 解析）= QQ 协议层活着
    pub ws_up: bool,
    pub api_up: bool,
    pub dl: BotDlStats,
    /// 最近 50 条下载/归档消息，按 id 倒序
    pub recent: Vec<BotDlItem>,
    pub db_note: Option<String>,
}

pub(crate) fn get_settings(state: &State<AppState>) -> Result<BotSettings, String> {
    let conn = state.db.lock().map_err_str()?;
    let root = db::get_setting(&conn, BOT_ROOT_KEY)
        .map_err_str()?
        .unwrap_or_else(default_root);
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

/// 读 bot 的 config.json 里一个字符串键（不存在/解析失败返回 None）。
/// 数据目录、端口、NapCat 根目录都从这里来——config.json 是 bot 与 viewman
/// 两边的唯一事实来源，换机器只改这一个文件
pub(crate) fn bot_config_str(root: &str, key: &str) -> Option<String> {
    let raw = std::fs::read_to_string(Path::new(root).join("config.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get(key)?.as_str().map(str::to_string)
}

/// bot 的数据目录：config.json 的 data_dir 说了算，缺省退 root。
/// napcat.rs 的日志路径也从这里派生
pub(crate) fn data_dir(root: &str) -> std::path::PathBuf {
    match bot_config_str(root, "data_dir") {
        Some(dir) if !dir.trim().is_empty() => Path::new(&dir).to_path_buf(),
        _ => Path::new(root).to_path_buf(),
    }
}

/// 从 URL 里取端口（ws://127.0.0.1:9501/ -> 9501），解析不出用 fallback
pub(crate) fn port_of(url: &str, fallback: u16) -> u16 {
    url.split("//")
        .nth(1)
        .and_then(|host| host.split('/').next())
        .and_then(|host| host.rsplit(':').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or(fallback)
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

/// 一次 netstat 查两个端口是否在监听（实现在 napcat.rs，省一次进程派生）
fn ports_up(ws: u16, api: u16) -> (bool, bool) {
    super::napcat::ports_up(ws, api)
}

/// 从连接读下载积压计数；拆出来是为单测可直接喂内存库
fn read_stats(conn: &rusqlite::Connection) -> Result<BotDlStats, String> {
    let mut dl = BotDlStats::default();
    let mut stmt = conn
        .prepare(
            "SELECT dl, COUNT(*) FROM messages
             WHERE dl IN ('pending','busy','dead') GROUP BY dl",
        )
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
            "dead" => dl.dead = n,
            _ => {}
        }
    }
    Ok(dl)
}

/// 最近 50 条有下载动作的消息（含转发归档行与转发明细行）；
/// 纯文本直发消息（dl=none 且无 parent）不进列表
fn read_recent(conn: &rusqlite::Connection) -> Result<Vec<BotDlItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, COALESCE(NULLIF(name, ''), substr(COALESCE(text, ''), 1, 80), ''),
                    bytes, dedup, err, dl
             FROM messages WHERE dl != 'none' OR parent IS NOT NULL
             ORDER BY id DESC LIMIT 50",
        )
        .map_err_str()?;
    let mut out = Vec::new();
    for row in stmt
        .query_map([], |r: &rusqlite::Row| {
            Ok(BotDlItem {
                id: r.get(0)?,
                kind: r.get(1)?,
                label: r
                    .get::<_, Option<String>>(2)?
                    .unwrap_or_default(),
                size: r.get(3)?,
                dedup: r.get(4)?,
                err: r.get(5)?,
                dl: r.get(6)?,
            })
        })
        .map_err_str()?
    {
        out.push(row.map_err_str()?);
    }
    Ok(out)
}

/// 只读打开 bot.db（WAL 库，bot 运行中也在写）拿积压计数与最近列表；读连接短轮询 5s 等锁，不自持
fn read_bot_db(db_path: &str) -> Result<(BotDlStats, Vec<BotDlItem>), String> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let conn = rusqlite::Connection::open_with_flags(db_path, flags).map_err_str()?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err_str()?;
    Ok((read_stats(&conn)?, read_recent(&conn)?))
}

#[tauri::command]
pub async fn get_bot_status(app: tauri::AppHandle) -> Result<BotStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let settings = get_settings(&state)?;
        let (bot_pid, riskwatch_pid) = find_pids();
        // 锁探针为准，命令行只用来拿 PID 展示；两者不一致时以锁为准
        let bot_running = bot_lock_held(&data_dir(&settings.root));
        // 端口从 bot 的 config.json（ws_url/api_url）解析，不再写死 9501/9502
        let ws_port = port_of(&bot_config_str(&settings.root, "ws_url")
            .unwrap_or_default(), 9501);
        let api_port = port_of(&bot_config_str(&settings.root, "api_url")
            .unwrap_or_default(), 9502);
        let (ws_up, api_up) = ports_up(ws_port, api_port);

        let mut dl = BotDlStats::default();
        let mut recent: Vec<BotDlItem> = Vec::new();
        let db_path = bot_db_path(&settings.root);
        let mut db_note: Option<String> = None;
        match std::fs::metadata(&db_path) {
            Err(_) => db_note = Some(format!("数据库不存在: {}", db_path.display())),
            Ok(_) => match read_bot_db(&db_path.to_string_lossy()) {
                Ok((d, list)) => {
                    dl = d;
                    recent = list;
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
            recent,
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
            "CREATE TABLE messages(id INTEGER PRIMARY KEY, dl TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(id, dl)
             VALUES (1,'pending'), (2,'busy'), (3,'dead'), (4,'ok'), (5,'none')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn stats_count_only_backlog_states() {
        let conn = test_db();
        let dl = read_stats(&conn).unwrap();
        assert_eq!((dl.pending, dl.busy, dl.dead), (1, 1, 1));
    }

    #[test]
    fn port_of_parses_urls_with_fallback() {
        assert_eq!(port_of("ws://127.0.0.1:9501/", 1), 9501);
        assert_eq!(port_of("http://127.0.0.1:9502", 1), 9502);
        assert_eq!(port_of("not a url", 9501), 9501);
        assert_eq!(port_of("", 9502), 9502);
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
