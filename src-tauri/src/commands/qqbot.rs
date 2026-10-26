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
/// 只留徽标用的三项）。dead 是累计死信 backlog（复检跑满前不删），面板不展示它——
/// 展示 dead_today，累计值天天挂着毫无意义
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BotDlStats {
    pub pending: i64,
    pub busy: i64,
    pub dead: i64,
    /// 今天才转死的条数（按 messages.dead_at 归自然日，旧积压不计）
    pub dead_today: i64,
}

/// 下载汇总：媒体文件（image/video/file，dl='ok'）的累计与今日完成量。
/// 转发父行 kind='forward' 不计（bytes 在明细行上，父子都算会翻倍）
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BotDlSummary {
    /// 累计已下载媒体文件数
    pub ok: i64,
    /// 累计已下载字节
    pub ok_bytes: i64,
    /// 今日（本地自然日）完成数
    pub today: i64,
    /// 今日完成字节
    pub today_bytes: i64,
    /// 今日重复跳过的媒体数（复用现有文件，不产生新下载）。每日清零：
    /// 累计值只会一直变大，面板要的是「今天省下多少下载」
    pub dedup_today: i64,
}

/// 下载列表一行：当日有下载动作的消息（含转发归档行与其转发明细行）
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
    /// 转发明细行指向所属转发行；顶层消息为空
    pub parent: Option<i64>,
    /// 明细行的内层发送者昵称（转发里不同人发的图靠它区分）
    pub sender: Option<String>,
    /// 本次尝试已落盘字节：仅 dl='busy' 时有意义，面板据此显示实时进度。
    /// 转发父行是块内聚合值。老库缺列时为 NULL
    pub dl_bytes: Option<i64>,
    /// Content-Length（来源未给或转发行聚合时为 NULL → 只显示已下载量）
    pub dl_total: Option<i64>,
    /// 消息时间（Unix 秒）。顶层行=收到的时刻；转发明细行=内层消息的原始
    /// 发送时间，可能早于今天（面板据此对跨日的补显日期）
    pub ts: Option<i64>,
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
    /// 下载汇总（累计 + 今日完成量、今日重复跳过）
    pub summary: BotDlSummary,
    /// 当日全部有下载动作的消息，按 id 倒序（前端再按状态筛）
    pub today_items: Vec<BotDlItem>,
    pub db_note: Option<String>,
}

/// 轻量快照：只读 bot.db，不探测进程/端口。下载活跃期前端高频轮这个，
/// PowerShell/netstat 探测仍走 10 秒的全量 get_bot_status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotDlSnapshot {
    pub dl: BotDlStats,
    pub summary: BotDlSummary,
    pub today_items: Vec<BotDlItem>,
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
    // 有损解码：data_dir 里带中文（D:\QQ数据）时，严格 UTF-8 读不了会让整个
    // 路径退化，面板就会去 root 下找 bot.db / 日志
    let path = Path::new(root).join("config.json");
    let raw = super::napcat::read_to_string(&path.to_string_lossy())?;
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

pub(crate) fn bot_db_path(root: &str) -> std::path::PathBuf {
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

/// 解析 find_pids 的输出行：PowerShell 每行打 `bot <pid>` / `rw <pid>`。
/// 抽成纯函数是为了单测——曾经这里把第 0 段当 PID、第 1 段当标签，两列正好读反，
/// 兜底恒返回 (None, None)，「bot 在跑但面板显示未运行」就再也救不回来。
fn parse_pid_lines(out: &str) -> (Option<u32>, Option<u32>) {
    let (mut bot, mut rw) = (None, None);
    for line in out.lines() {
        let mut parts = line.splitn(2, ' ');
        let label = parts.next().unwrap_or("");
        let pid = parts.next().and_then(|p| p.trim().parse::<u32>().ok());
        match (label, pid) {
            ("bot", Some(p)) => bot = Some(p),
            ("rw", Some(p)) => rw = Some(p),
            _ => {}
        }
    }
    (bot, rw)
}

/// 按命令行匹配 python/pythonw 进程（start.bat 同款）：bot.py 与 riskwatch.py 各找 PID。
/// 别的 Python 程序不受影响；powershell 冷启动约一秒，配 10s 轮询无压力
fn find_pids() -> (Option<u32>, Option<u32>) {
    // 按整段文件名 `\bot.py` 匹配：只写 `*bot.py*` 会把 my_bot.py、bot.py.tmp 也算进来
    let script = "Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR Name='pythonw.exe'\" | Where-Object { $_.CommandLine -like '*\\riskwatch.py*' -or $_.CommandLine -like '*\\bot.py*' } | ForEach-Object { if ($_.CommandLine -like '*\\riskwatch.py*') { \"rw $($_.ProcessId)\" } else { \"bot $($_.ProcessId)\" } }";
    let out = silent_command("powershell")
        .args(["-NoProfile", "-Command", script])
        .output();
    match out {
        Ok(o) => parse_pid_lines(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => (None, None),
    }
}

#[cfg(windows)]
mod winpid {
    // PID 校验用的最小 FFI：OpenProcess + QueryFullProcessImageNameW +
    // NtQueryInformationProcess。拿可执行文件路径与命令行是为了防 PID 复用误杀
    // （pidfile 里的 PID 可能已被新进程顶用：换过 Python 的残留、或 replay.py /
    // search.py 这种同解释器的工具恰好吃到同一个号）
    #[link(name = "kernel32")]
    extern "system" {
        pub fn OpenProcess(desired: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        pub fn QueryFullProcessImageNameW(
            h: *mut core::ffi::c_void,
            flags: u32,
            buf: *mut u16,
            len: *mut u32,
        ) -> i32;
        pub fn CloseHandle(h: *mut core::ffi::c_void) -> i32;
        pub fn ReadProcessMemory(
            h: *mut core::ffi::c_void,
            base: *const core::ffi::c_void,
            buf: *mut core::ffi::c_void,
            size: usize,
            read: *mut usize,
        ) -> i32;
    }
    // ProcessBasicInformation = 0，拿 PEB 地址；随后每一处远程内存都必须走
    // ReadProcessMemory（它需要 PROCESS_VM_READ，OpenProcess 时一并申请）
    #[link(name = "ntdll")]
    extern "system" {
        pub fn NtQueryInformationProcess(
            h: *mut core::ffi::c_void,
            class: u32,
            info: *mut core::ffi::c_void,
            len: u32,
            ret: *mut u32,
        ) -> i32;
    }
}

/// 从目标进程读一块内存到本地字节切片。地址为 0、权限不足、短读（目标恰好退出）
/// 一律返回 false——绝不把远程地址解引用成本进程指针。
/// 历史教训：曾经直接 `*(peb + 0x20)` 读远程 PEB，同位宽进程的 PEB VA 平时"碰巧"
/// 也映射在本进程里所以没炸；目标进程一退出、该 VA 解除映射，下一次轮询就
/// 0xC0000005 打死主进程（2026-10-23 十个崩溃 dump 全在同一指令 peb+0x20）。
#[cfg(windows)]
unsafe fn rpm_into(
    h: *mut core::ffi::c_void,
    base: usize,
    dst: &mut [u8],
) -> bool {
    if base == 0 {
        return false;
    }
    let mut done = 0usize;
    winpid::ReadProcessMemory(
        h,
        base as *const core::ffi::c_void,
        dst.as_mut_ptr().cast(),
        dst.len(),
        &mut done,
    ) != 0
        && done == dst.len()
}

/// 读目标进程里的一个指针/usize 值
#[cfg(windows)]
unsafe fn rpm_usize(h: *mut core::ffi::c_void, base: usize) -> Option<usize> {
    let mut buf = [0u8; 8];
    rpm_into(h, base, &mut buf).then(|| usize::from_le_bytes(buf))
}

/// 只查映像名（QueryFullProcessImageNameW）用 LIMITED 就够
const PROCESS_QUERY_LIMITED: u32 = 0x1000; // PROCESS_QUERY_LIMITED_INFORMATION
/// NtQueryInformationProcess 走它；读 PEB 链还要 PROCESS_VM_READ——两者一起申请
const PROCESS_QUERY_VM: u32 = 0x1000 | 0x10; // PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ

/// 进程命令行。取不到返回 None：调用方据此**拒绝**使用这个 PID——
/// 读不到就不认，宁可退回 PowerShell 兜底，也不要凭一个无法核实的号去 taskkill。
///
/// PEB → ProcessParameters → CommandLine 每一跳都必须经 ReadProcessMemory：
/// 这些是**目标进程**的地址，直接解引用只有在本进程恰好映射了同一 VA 时才"碰巧"
/// 不炸；目标一退出（bot 重启、被杀、PID 复用的间隙）VA 解除映射，下一次 10s
/// 轮询就是 0xC0000005 打死主进程。2026-10-23 的十个崩溃 dump 全停在旧实现的
/// `read_unaligned(peb.add(0x20))` 这一条指令上。
#[cfg(windows)]
fn process_command_line(pid: u32) -> Option<String> {
    unsafe {
        let h = winpid::OpenProcess(PROCESS_QUERY_VM, 0, pid);
        if h.is_null() {
            return None;
        }
        let result = read_cmdline(h);
        winpid::CloseHandle(h);
        result
    }
}

/// h 必须是带 PROCESS_VM_READ 的活句柄。任何一跳读不到都返回 None，绝不 panic。
#[cfg(windows)]
unsafe fn read_cmdline(h: *mut core::ffi::c_void) -> Option<String> {
    use std::os::windows::ffi::OsStringExt;
    /// RTL_USER_PROCESS_PARAMETERS::CommandLine 在 x64 上的偏移
    const CMDLINE_OFFSET: usize = 0x70;
    /// UNICODE_STRING 布局：Length(2) + MaximumLength(2) + padding(4) + Buffer(8)
    const US_LEN_AT: usize = 0;
    const US_BUF_AT: usize = 8;
    /// PEB->ProcessParameters 在 x64 上偏移 0x20
    const PEB_PARAMS_AT: usize = 0x20;

    // PROCESS_BASIC_INFORMATION：PebBaseAddress 在第 2 个指针；用 usize 数组接，8 字节对齐
    let mut pbi = [0usize; 6];
    let mut ret = 0u32;
    let status = winpid::NtQueryInformationProcess(
        h,
        0,
        pbi.as_mut_ptr().cast(),
        std::mem::size_of_val(&pbi) as u32,
        &mut ret,
    );
    if status != 0 {
        return None;
    }
    let peb = pbi[1];
    // PEB->ProcessParameters
    let params = rpm_usize(h, peb.checked_add(PEB_PARAMS_AT)?)?;
    // RTL_USER_PROCESS_PARAMETERS::CommandLine (UNICODE_STRING)
    let us = params.checked_add(CMDLINE_OFFSET)?;
    let mut len_buf = [0u8; 2];
    if !rpm_into(h, us.checked_add(US_LEN_AT)?, &mut len_buf) {
        return None;
    }
    let len = u16::from_le_bytes(len_buf) as usize;
    let mut ptr_buf = [0u8; 8];
    if !rpm_into(h, us.checked_add(US_BUF_AT)?, &mut ptr_buf) {
        return None;
    }
    let buf = usize::from_le_bytes(ptr_buf);
    if buf == 0 || len == 0 || len > 0x8000 {
        return None;
    }
    // 只读 Length 个字节（不含结尾 NUL；Length 为偶数，按 u16 个数取 len/2）
    let mut chars = vec![0u16; len / 2];
    let dst = std::slice::from_raw_parts_mut(chars.as_mut_ptr() as *mut u8, (len / 2) * 2);
    if !rpm_into(h, buf, dst) {
        return None;
    }
    Some(std::ffi::OsString::from_wide(&chars).to_string_lossy().into_owned())
}

/// 进程命令行是否指向某个脚本（`<python 路径> D:\...\bot.py`）。
/// 比对整段文件名而不是 `contains("bot.py")`：别的目录下的 `my_bot.py`、
/// `bot.py.tmp` 都不该命中；前面必须是路径分隔符（`\` / `/` / 盘符 `:`），
/// 后面必须是空白/引号/行尾。比较用小写，Windows 路径不区分大小写
fn cmdline_runs_script(cmdline: &str, script: &str) -> bool {
    let lower = cmdline.to_ascii_lowercase();
    let script = script.to_ascii_lowercase();
    lower.match_indices(&script).any(|(i, m)| {
        let end = i + m.len();
        // 结尾必须是空白/引号/行尾，排除 bot.py.tmp、my_bot.py 之类
        let tail_ok = lower[end..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || c == '"' || c == '\'');
        // 前面是目录分隔符（\ 或 /），或正好是盘符（D:bot.py，Windows 也认这种写法）
        let head_ok = match lower[..i].chars().next_back() {
            Some(c) => c == '\\' || c == '/' || c == ':',
            None => false,
        };
        tail_ok && head_ok
    })
}

/// 进程映像与配置的 python 是不是同一个解释器。
///
/// 三种情况都算同一个，其余一律不算：
/// - 同一路径（大小写不敏感）；
/// - 末尾文件名相同（settings 里常是 `D:\Python311\python.exe`，start.bat 却用
///   `%PYW%` 即同目录的 pythonw.exe；盘符或目录写拍子不一样时也仍然认）；
/// - 进程是 pythonw.exe（没加启动参数时 pythonw 会直接拉起同目录的 python.exe，
///   真正跑脚本的是子进程，父进程的命令行里也带着 bot.py，不认它就会同时命中两个 PID，
///   面板的 bot_pid 会跟着来回跳）。
///
/// 命令行校验（pid_runs）不因这里的放宽而失效：启动参数里的 `-c import runpy`
/// 里没有 bot.py 的路径，命中不了
fn same_python(image: &str, expected: &str) -> bool {
    let file_name = |p: &str| {
        p.to_ascii_lowercase()
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    };
    image.eq_ignore_ascii_case(expected)
        || file_name(image) == file_name(expected)
        || file_name(image) == "pythonw.exe"
}

/// PID 是否存活、可执行文件就是配置的 python、且命令行跑的是指定脚本
/// （pythonw.exe 视同 python.exe，start.bat 用 pythonw 跑 bot.py、python 跑 riskwatch.py）。
///
/// 三道都要过：pidfile 在进程退出后**不会**被删（进程被杀时没机会清理），
/// 残留的号迟早会被别的进程复用——小号最先被分给刚启动的进程，也就是用户
/// 再点一次「启动」时的 python.exe。只验可执行文件的话，replay.py / search.py
/// 这种同解释器工具一旦吃到这个号，面板就会误报「bot 在运行」，停止按钮还会
/// 去 taskkill 一个不相干的进程
fn pid_runs(pid: u32, expected: &Path, script: &str) -> bool {
    #[cfg(not(windows))]
    {
        let _ = (pid, expected, script);
        false
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        unsafe {
            let h = winpid::OpenProcess(PROCESS_QUERY_LIMITED, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok =
                winpid::QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
            winpid::CloseHandle(h);
            if !ok {
                return false;
            }
            let img = std::ffi::OsString::from_wide(&buf[..len as usize]);
            if !same_python(&img.to_string_lossy(), &expected.to_string_lossy()) {
                return false;
            }
        }
        process_command_line(pid).is_some_and(|cl| cmdline_runs_script(&cl, script))
    }
}

/// 读 pidfile（bot.py / riskwatch.py 启动时写）
fn read_pid_file(data_dir: &Path, name: &str) -> Option<u32> {
    std::fs::read_to_string(data_dir.join(name))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// PID 解析：pidfile 优先（毫秒级、不派生进程），校验进程确实是配置的 python
/// 且命令行跑的是对应脚本（见 pid_runs）；pidfile 缺失（旧版 bot 还没重启过）
/// 或校验不过时退回 PowerShell 按命令行匹配。兜底是有原因的：GUI 环境里 spawn
/// 的进程 CommandLine 可能读不到，曾导致「bot 明明在跑，停止按钮却说未在运行」。
fn resolve_pids(root: &str, python: &str) -> (Option<u32>, Option<u32>) {
    let dd = data_dir(root);
    let expected = Path::new(python);
    let mut bot = read_pid_file(&dd, "bot.pid").filter(|p| pid_runs(*p, expected, "bot.py"));
    let mut rw =
        read_pid_file(&dd, "riskwatch.pid").filter(|p| pid_runs(*p, expected, "riskwatch.py"));
    // 只在 bot 缺席时才跑 PowerShell：riskwatch 是可选组件（没在跑是常态），
    // 为了补它每 10 秒派生一次 powershell 不划算
    if bot.is_none() {
        let (fb, fr) = find_pids();
        bot = bot.or(fb);
        rw = rw.or(fr);
    }
    (bot, rw)
}

/// 状态探测用的 PID 解析：resolve_pids 之外再多做两道剔除。
///
/// 1. **残留 pidfile 要清**：bot 被 taskkill / 崩溃时没机会删自己的 pidfile。
///    每轮探测到这个文件已经指向别的进程（或进程已经不在），就顺手删掉，
///    免得它一直躺在那儿等被复用。
/// 2. **bot 没在跑就别报 PID**：前端的 running 是 `bot_running || bot_pid != null`，
///    残留的号一旦被任意 python 进程顶用（PowerShell 兜底匹配也不校验路径），
///    面板会显示「运行中」。以 bot.lock 探针为准：锁没被占，bot_pid 一律置空。
fn probe_pids(root: &str, python: &str) -> (Option<u32>, Option<u32>) {
    let dd = data_dir(root);
    let (bot, rw) = resolve_pids(root, python);
    // 解析不出来 = 文件里那个号已不是本脚本的进程（进程没了或号被复用），删掉它。
    // 文件不存在时 remove_file 也是无害的 Err，忽略即可
    for (file, pid) in [("bot.pid", bot), ("riskwatch.pid", rw)] {
        if pid.is_none() {
            let _ = std::fs::remove_file(dd.join(file));
        }
    }
    // 锁没被占 = bot 没在跑。PID 一律置空，免得残留的号让界面显示「运行中」；
    // 锁被占但没解析出号时也照实返回 None——界面靠 bot_running 显示「运行中」
    let bot_running = bot_lock_held(&dd);
    (bot.filter(|_| bot_running), rw)
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
    // bot.db 由 python 侧写，老库可能还没升 dead_at 列；缺列就当 0，
    // 别让整块面板跟着 read_stats 一起报错。口径与当日列表里「今天转死」那一支同源
    if has_messages_col(conn, "dead_at") {
        dl.dead_today = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE dl='dead' AND dead_at >= ?1",
                [today_start_ts()],
                |r| r.get(0),
            )
            .map_err_str()?;
    }
    Ok(dl)
}

/// messages 表是否有某列（PRAGMA 元数据，代价可忽略）
fn has_messages_col(conn: &rusqlite::Connection, col: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM pragma_table_info('messages') WHERE name=?",
        [col],
        |_| rusqlite::Result::Ok(()),
    )
    .is_ok()
}

/// 今天 00:00（本地时区）的 Unix 秒。所有「今日」口径都以它为准。
/// 用它做 ts >= 起点 而不是每行 strftime(...,'localtime') 比日期串：
/// 后者让 messages_ts 索引彻底失效，2 秒一轮的高频快照会退化成全表扫。
fn today_start_ts() -> i64 {
    let now = chrono::Local::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| midnight.and_local_timezone(chrono::Local).single())
        .map(|dt| dt.timestamp())
        // 极少数时区本地 00:00 不存在（夏令时前跳）：退到 24 小时前，
        // 宁可多算一天，也不要让面板在午夜前后整个空掉
        .unwrap_or_else(|| now.timestamp() - 86_400)
}

/// 当日下载列表：今天有下载动作的消息一条不落（含转发归档行与转发明细行），按 id 倒序。
///
/// 转发明细行的 `ts` 是**内层消息的原始时间**（bot.py 落库时就是这么写的：
/// 转发是今天收到的，里面的图可能是三个月前发的）。按 ts 归日会让今天收到的转发
/// 在列表里只剩孤零零一个父行、里面 700 张图一条都不显示——实测今天 19 个转发
/// 705 条明细全部漏掉。**明细行按父行归日**：它是被哪条转发带进来的，就记在哪天。
///
/// 归日口径与「今日已下载」「今日失败」两个计数严格对齐，一行满足其一即可：
/// `ts >= 今天 00:00`（顶层消息今天到的，含今早排队、下午才下完的）；
/// 或 `dl='dead' 且 dead_at >= 今天 00:00`（今天才转死的——几天前的消息在今日复检中
/// 失败时也能看见，与 dead_today 对得上；每日复检再次失败不刷新 dead_at，
/// 见 db.bump_dl，所以旧积压不会被天天重计）。
///
/// 不设条数上限：列表每日自然清零，单日行数与人手能扫的量级相当。
/// 纯文本直发消息（dl='none' 且无 parent）不进列表——这里只列下载/归档动作。
///
/// 拆成三条查询而不是一条 OR，是为了让索引用得上：带 `ORDER BY id DESC` 的单查询
/// 会被 sqlite 判成「倒着扫表更便宜」，6 万行实测 25ms/次，2 秒一轮的快照纯浪费。
/// 三支各自命中索引（顶层走 messages_ts，明细走父行的 ts，死信走 messages_due），
/// 结果在内存里合并去重。
fn read_today_items(conn: &rusqlite::Connection) -> Result<Vec<BotDlItem>, String> {
    let today = today_start_ts();
    // 实时进度两列是老库可能没有的（db.py 启动时才迁移）；缺列补 NULL，
    // 面板只在 busy 行读它们，拿不到就当没有进度
    let has_prog = has_messages_col(conn, "dl_bytes");
    // 接收时间统一走 LEFT JOIN 取 COALESCE(p.ts, m.ts)：顶层行父为空、落回自身 ts；
    // 转发明细行显示的是**父转发收到的时刻**（明细自身 ts 是内层消息的原始发送时间，
    // 可能是几个月前——那不是这条内容进入本机的时间）。LEFT JOIN 对驱动表扫描无影响
    // （顶层仍走 messages_ts，父行按 rowid 点查），老库也都有这两列，无需缺列兜底
    let cols = format!(
        "SELECT m.id, m.kind, COALESCE(NULLIF(m.name, ''), \
         substr(COALESCE(m.text, ''), 1, 80), ''), m.bytes, m.dedup, \
         m.err, m.dl, m.parent, m.sender_nick{}, COALESCE(p.ts, m.ts) \
         FROM messages m LEFT JOIN messages p ON p.id = m.parent",
        if has_prog { ", m.dl_bytes, m.dl_total" } else { ", NULL, NULL" }
    );
    let cols = cols.as_str();
    let mut out = Vec::new();
    let mut push = |stmt: &mut rusqlite::Statement| -> Result<(), String> {
        let rows = stmt
            .query_map([today], |r: &rusqlite::Row| {
                Ok(BotDlItem {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    label: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    size: r.get(3)?,
                    dedup: r.get(4)?,
                    err: r.get(5)?,
                    dl: r.get(6)?,
                    parent: r.get(7)?,
                    sender: r.get(8)?,
                    dl_bytes: r.get(9)?,
                    dl_total: r.get(10)?,
                    ts: r.get(11)?,
                })
            })
            .map_err_str()?;
        for row in rows {
            out.push(row.map_err_str()?);
        }
        Ok(())
    };

    // 今天到的消息。**不要**加 `parent IS NULL`：那条会让 sqlite 改用 messages_parent
    // 索引扫出全部 5 万条顶层行（28ms），而走 messages_ts 只要 0.16ms。顺带命中
    // 「ts 是今天的明细行」也无妨——转发里的消息时间必然不晚于转发本身，三支重叠
    // 由下面的去重兜住。
    let mut stmt = conn
        .prepare(&format!("{cols} WHERE m.dl != 'none' AND m.ts >= ?1"))
        .map_err_str()?;
    push(&mut stmt).map_err_str()?;

    // 今天收到的转发带进来的全部明细行（含文本/嵌套转发这些 dl='none' 的）。
    // 父行关联已在 cols 里 LEFT JOIN 好，这里只差 WHERE；
    // 需要 messages_parent 索引（db.py 启动时建），否则这条会退化成全表扫 27ms
    let mut stmt = conn
        .prepare(&format!("{cols} WHERE p.ts >= ?1"))
        .map_err_str()?;
    push(&mut stmt).map_err_str()?;

    // 昨天到的消息在今日复检中转死，也要出现在今天的列表里（与 dead_today 同口径）。
    // bot.db 由 python 侧建，老库可能还没有 dead_at 列；缺列就只按消息时间归日
    if has_messages_col(conn, "dead_at") {
        let mut stmt = conn
            .prepare(&format!("{cols} WHERE m.dl = 'dead' AND m.dead_at >= ?1"))
            .map_err_str()?;
        push(&mut stmt).map_err_str()?;
    }

    // 三支可能重复命中同一行（今天收到的、今天死的），按 id 去重。
    // 排序键取 COALESCE(parent, id) 倒序（新消息在前），块内按 id 正序：
    // 父行 id 最小排最前，转发明细跟在父行下面、按转发里的先后铺开
    let mut seen = std::collections::HashSet::with_capacity(out.len());
    out.retain(|it| seen.insert(it.id));
    out.sort_by(|a, b| {
        let ka = a.parent.unwrap_or(a.id);
        let kb = b.parent.unwrap_or(b.id);
        kb.cmp(&ka).then(a.id.cmp(&b.id))
    });
    Ok(out)
}

/// 下载汇总：一次聚合出累计/今日完成量与今日重复跳过数。
/// 今日口径同 today_start_ts（消息时间 ts，本地自然日）
fn read_summary(conn: &rusqlite::Connection) -> Result<BotDlSummary, String> {
    let mut s = BotDlSummary::default();
    let today = today_start_ts();
    let mut stmt = conn
        .prepare(
            "SELECT COUNT(*), COALESCE(SUM(bytes),0),
                    COALESCE(SUM(CASE WHEN ts >= ?1 THEN 1 END),0),
                    COALESCE(SUM(CASE WHEN ts >= ?1 THEN COALESCE(bytes,0) END),0)
             FROM messages WHERE dl='ok' AND kind IN ('image','video','file')",
        )
        .map_err_str()?;
    let row = stmt
        .query_row([today], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
        })
        .map_err_str()?;
    s.ok = row.0;
    s.ok_bytes = row.1;
    s.today = row.2;
    s.today_bytes = row.3;
    s.dedup_today = conn
        .query_row(
            "SELECT COUNT(*) FROM messages
             WHERE dedup > 0 AND kind != 'forward' AND ts >= ?1",
            [today],
            |r| r.get(0),
        )
        .map_err_str()?;
    Ok(s)
}

/// 只读打开 bot.db（WAL 库，bot 运行中也在写）拿积压计数、汇总与当日列表；读连接短轮询 5s 等锁，不自持
fn read_bot_db(db_path: &str) -> Result<(BotDlStats, BotDlSummary, Vec<BotDlItem>), String> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let conn = rusqlite::Connection::open_with_flags(db_path, flags).map_err_str()?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err_str()?;
    Ok((read_stats(&conn)?, read_summary(&conn)?, read_today_items(&conn)?))
}

/// 轻量快照命令：只有一次 sqlite 只读 + settings 读取，毫秒级，供活跃期 2s 轮询
#[tauri::command]
pub async fn get_bot_dl_snapshot(app: tauri::AppHandle) -> Result<BotDlSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let settings = get_settings(&state)?;
        let db_path = bot_db_path(&settings.root);
        let (dl, summary, today_items) = read_bot_db(&db_path.to_string_lossy())?;
        Ok(BotDlSnapshot { dl, summary, today_items })
    })
    .await
    .map_err_str()?
}

#[tauri::command]
pub async fn get_bot_status(app: tauri::AppHandle) -> Result<BotStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let settings = get_settings(&state)?;
        // PID 解析走 pidfile（校验进程身份），PowerShell 只兜底；顺带清残留 pidfile
        let (bot_pid, riskwatch_pid) = probe_pids(&settings.root, &settings.python);
        // 锁探针为准，命令行只用来拿 PID 展示；两者不一致时以锁为准
        let bot_running = bot_lock_held(&data_dir(&settings.root));
        // 端口从 bot 的 config.json（ws_url/api_url）解析，不再写死 9501/9502
        let ws_port = port_of(&bot_config_str(&settings.root, "ws_url")
            .unwrap_or_default(), 9501);
        let api_port = port_of(&bot_config_str(&settings.root, "api_url")
            .unwrap_or_default(), 9502);
        let (ws_up, api_up) = ports_up(ws_port, api_port);

        let mut dl = BotDlStats::default();
        let mut summary = BotDlSummary::default();
        let mut today_items: Vec<BotDlItem> = Vec::new();
        let db_path = bot_db_path(&settings.root);
        let mut db_note: Option<String> = None;
        match std::fs::metadata(&db_path) {
            Err(_) => db_note = Some(format!("数据库不存在: {}", db_path.display())),
            Ok(_) => match read_bot_db(&db_path.to_string_lossy()) {
                Ok((d, s, list)) => {
                    dl = d;
                    summary = s;
                    today_items = list;
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
            summary,
            today_items,
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
pub async fn stop_bot(state: State<'_, AppState>) -> Result<String, String> {
    let s = get_settings(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (bot, rw) = resolve_pids(&s.root, &s.python);
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
        // 进程被杀时没机会自己删 pidfile，停完顺手清掉，免得残留的号被复用后误报
        let dd = data_dir(&s.root);
        for file in ["bot.pid", "riskwatch.pid"] {
            let _ = std::fs::remove_file(dd.join(file));
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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        conn.execute_batch(
            "CREATE TABLE messages(id INTEGER PRIMARY KEY, dl TEXT, dead_at INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(id, dl, dead_at) VALUES
             (1,'pending',NULL), (2,'busy',NULL), (3,'dead',?1), (4,'ok',NULL),
             (5,'none',NULL), (6,'dead',?2), (7,'dead',NULL)",
            rusqlite::params![now, now - 3 * 86400],
        )
        .unwrap();
        conn
    }

    #[test]
    fn stats_count_only_backlog_states() {
        let conn = test_db();
        let dl = read_stats(&conn).unwrap();
        assert_eq!((dl.pending, dl.busy, dl.dead), (1, 1, 3));
    }

    /// 今日失败只数「今天转死」的行：三天前死的和没有 dead_at 的旧积压都不算
    #[test]
    fn dead_today_counts_rows_that_died_today_only() {
        let conn = test_db();
        let dl = read_stats(&conn).unwrap();
        assert_eq!(dl.dead_today, 1);
    }

    /// bot.db 由 python 侧建，老库可能还没有 dead_at 列。缺列时 dead_today 退成 0，
    /// 不能让整个 read_stats 报错、连累整块面板
    #[test]
    fn stats_tolerates_db_without_dead_at_column() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE messages(id INTEGER PRIMARY KEY, dl TEXT);")
            .unwrap();
        conn.execute("INSERT INTO messages(id, dl) VALUES (1,'dead')", [])
            .unwrap();
        let dl = read_stats(&conn).unwrap();
        assert_eq!((dl.dead, dl.dead_today), (1, 0));
    }

    /// 汇总只数媒体文件（dl='ok' 的 image/video/file）；转发父行不算 bytes，
    /// dedup 只数非 forward 行避免父子双计。昨日的行不进今日口径——这就是「每日清零」
    #[test]
    fn summary_counts_media_files_only() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE messages(id INTEGER PRIMARY KEY, kind TEXT, dl TEXT,
                                   bytes INTEGER, dedup INTEGER NOT NULL DEFAULT 0, ts INTEGER);",
        )
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO messages(id, kind, dl, bytes, dedup, ts) VALUES
             (1,'image','ok',100,0,?1),
             (2,'video','ok',200,0,?1),
             (3,'file','ok',50,1,?1),
             (4,'text','none',NULL,0,?1),
             (5,'forward','ok',NULL,3,?1),
             (6,'image','dead',NULL,0,?1),
             (7,'image','ok',999,1,?2),
             (8,'file','ok',888,0,?2)",
            rusqlite::params![now, now - 3 * 86400],
        )
        .unwrap();
        let s = read_summary(&conn).unwrap();
        assert_eq!(s.ok, 5, "累计含昨日两条");
        assert_eq!(s.ok_bytes, 2237);
        assert_eq!(s.today, 3);
        assert_eq!(s.today_bytes, 350);
        assert_eq!(s.dedup_today, 1, "昨日那条重复跳过不计入今日");
    }

/// 当日列表：今日消息一条不落，昨日的行只在「今天才转死」时回来；纯文本直发
    /// （dl='none' 且无 parent）不进列表；**转发明细行按父行归日**——它 ts 是内层
    /// 消息的旧时间，跟着父行才看得见今天转发里下到的图
    #[test]
    fn today_items_cover_the_whole_day_and_recent_deaths() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE messages(id INTEGER PRIMARY KEY, kind TEXT, dl TEXT, name TEXT,
                                   text TEXT, bytes INTEGER, dedup INTEGER NOT NULL DEFAULT 0,
                                   err TEXT, parent INTEGER, sender_nick TEXT, ts INTEGER,
                                   dead_at INTEGER, dl_bytes INTEGER NOT NULL DEFAULT 0,
                                   dl_total INTEGER);",
        )
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let old = now - 3 * 86400;
        conn.execute(
            "INSERT INTO messages(id,kind,dl,name,text,bytes,dedup,err,parent,sender_nick,ts,dead_at) VALUES
             (1,'image','ok','a.jpg',NULL,10,0,NULL,NULL,NULL,?1,NULL),
             (2,'forward','ok',NULL,'聊天记录',NULL,2,NULL,NULL,NULL,?1,NULL),
             (3,'image','none',NULL,'转发里的图',20,0,NULL,2,'小明',?1,NULL),
             (4,'text','none',NULL,'纯文本',NULL,0,NULL,NULL,NULL,?1,NULL),
             (5,'image','dead',NULL,NULL,NULL,0,'boom',NULL,NULL,?2,?3),
             (6,'image','dead',NULL,NULL,NULL,0,'boom',NULL,NULL,?2,?4),
             (7,'image','ok','y.jpg',NULL,30,0,NULL,NULL,NULL,?2,NULL),
             (8,'image','dead',NULL,NULL,NULL,0,'boom',NULL,NULL,?1,?3),
             (9,'image','ok','old.jpg',NULL,40,0,NULL,2,'小红',?2,NULL),
             (10,'image','ok','x.jpg',NULL,50,0,NULL,11,NULL,?2,NULL),
             (11,'forward','ok',NULL,'昨天的转发',NULL,0,NULL,NULL,NULL,?2,NULL)",
            rusqlite::params![now, old, now, old],
        )
        .unwrap();
        // 1 改成下载中并带实时进度：busy 行要把 dl_bytes/dl_total 透出给面板
        conn.execute(
            "UPDATE messages SET dl='busy', dl_bytes=12345, dl_total=98765 WHERE id=1",
            [],
        )
        .unwrap();
        let items = read_today_items(&conn).unwrap();
        let it1 = items.iter().find(|it| it.id == 1).unwrap();
        assert_eq!(it1.dl_bytes, Some(12345));
        assert_eq!(it1.dl_total, Some(98765));
        // 接收时间：顶层行就是自身 ts；明细行取父转发收到的时刻（内层旧时间被覆盖）；
        // 昨日消息今日转死的行仍是自身旧 ts（面板对跨日的补显日期）
        assert_eq!(it1.ts, Some(now));
        assert_eq!(items.iter().find(|it| it.id == 3).unwrap().ts, Some(now));
        assert_eq!(items.iter().find(|it| it.id == 9).unwrap().ts, Some(now), "明细 9 自身 ts 是旧的，跟父行 2 取今天");
        assert_eq!(items.iter().find(|it| it.id == 5).unwrap().ts, Some(old), "昨日转死行保留原始时间");
        // 非进度行取建表默认值（0/NULL），面板按 busy 才读它们
        assert_eq!(items.iter().find(|it| it.id == 8).unwrap().dl_bytes, Some(0));
        assert_eq!(items.iter().find(|it| it.id == 2).unwrap().dl_total, None);
        let ids: Vec<i64> = items.iter().map(|it| it.id).collect();
        // 排序键 COALESCE(parent,id) 倒序、块内 id 正序：8、5 各一组，转发 2 置顶、
        // 明细（3/9）跟在它下面，再下面 1。10/11 是昨天那批发来的转发与它的明细，
        // 今天没动静就不出现；4 是纯文本直发；8 两支都命中，去重后只出现一次
        assert_eq!(ids, vec![8, 5, 2, 3, 9, 1]);
        let detail = items.iter().find(|it| it.id == 3).unwrap();
        assert_eq!(detail.parent, Some(2));
        assert_eq!(detail.sender.as_deref(), Some("小明"));
        assert_eq!(items.iter().find(|it| it.id == 1).unwrap().parent, None);
    }

    /// 老库没有 dead_at 列时列表不能整块报错，退化成「只按消息时间归日」
    #[test]
    fn today_items_tolerate_db_without_dead_at_column() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE messages(id INTEGER PRIMARY KEY, kind TEXT, dl TEXT, name TEXT,
                                   text TEXT, bytes INTEGER, dedup INTEGER NOT NULL DEFAULT 0,
                                   err TEXT, parent INTEGER, sender_nick TEXT, ts INTEGER);",
        )
        .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO messages(id,kind,dl,name,text,bytes,dedup,err,parent,sender_nick,ts) VALUES
             (1,'image','ok','a.jpg',NULL,10,0,NULL,NULL,NULL,?1),
             (2,'image','ok','b.jpg',NULL,20,0,NULL,NULL,NULL,?2),
             (3,'image','ok','c.jpg',NULL,30,0,NULL,2,NULL,?2)",
            rusqlite::params![now, now - 3 * 86400],
        )
        .unwrap();
        let ids: Vec<i64> = read_today_items(&conn)
            .unwrap()
            .into_iter()
            .map(|it| it.id)
            .collect();
        // 3 是昨天的转发带进来的明细，跟着父行走，不出现
        assert_eq!(ids, vec![1]);
    }

    /// 今日起点是本地 00:00，且不晚于现在——否则「今日」会恒为 0
    #[test]
    fn today_start_is_local_midnight_and_not_in_the_future() {
        let start = today_start_ts();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(start <= now, "今日起点不能在未来");
        assert!(now - start < 86_400, "今日起点必须是今天，不能是更早的日子");
    }

    #[test]
    fn port_of_parses_urls_with_fallback() {
        assert_eq!(port_of("ws://127.0.0.1:9501/", 1), 9501);
        assert_eq!(port_of("http://127.0.0.1:9502", 1), 9502);
        assert_eq!(port_of("not a url", 9501), 9501);
        assert_eq!(port_of("", 9502), 9502);
    }

    /// pid_runs 拿当前测试进程自己验证：镜像路径一致**且**命令行跑着指定脚本才放行，
    /// 防止 pidfile 残留的号被别的进程顶用后误杀
    #[test]
    fn pid_runs_verifies_image_and_command_line() {
        let me = std::env::current_exe().unwrap();
        let cmd = process_command_line(std::process::id())
            .expect("测试进程自己的命令行应该读得到");
        // 文件名取自命令行里的最后一个 token（cargo test 的可执行文件名大小写不一定）
        let script = cmd.rsplit(['\\', '/', '"'])
            .find(|t| t.ends_with(".exe"))
            .unwrap_or("");
        assert!(!script.is_empty(), "命令行里应有测试程序的路径: {cmd}");
        assert!(pid_runs(std::process::id(), &me, script));
        assert!(!pid_runs(
            std::process::id(),
            &me.parent().unwrap().join("nope.exe"),
            script
        ));
        assert!(
            !pid_runs(std::process::id(), &me, "bot.py"),
            "镜像对但没跑 bot.py，不能当 bot"
        );
        assert!(!pid_runs(0, &me, script)); // PID 0 (System Idle) 不是可打开的进程
    }

    #[test]
    fn read_pid_file_parses_and_tolerates_missing() {
        let dir = std::env::temp_dir().join("viewman_pidfile_test");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("bot.pid"), " 12345\n").unwrap();
        assert_eq!(read_pid_file(&dir, "bot.pid"), Some(12345));
        assert_eq!(read_pid_file(&dir, "absent.pid"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_pid_lines_reads_label_then_pid() {
        // 实测 PowerShell 输出就是这两行；读反过一回，别再让它悄悄返回 (None, None)
        assert_eq!(
            parse_pid_lines("bot 23300\nrw 16484\n"),
            (Some(23300), Some(16484))
        );
        assert_eq!(parse_pid_lines("rw 7\n"), (None, Some(7)));
        assert_eq!(parse_pid_lines("bot notanumber\n"), (None, None));
        assert_eq!(parse_pid_lines(""), (None, None));
    }

    #[test]
    fn cmdline_script_match_is_whole_filename() {
        // 残留 pidfile 的号被 replay.py / 别的目录的 bot.py 吃到时不能算数
        let cl = "\"C:\\Python314\\python.exe\" \"D:\\opencword\\viewman\\qqbot\\bot.py\"";
        assert!(cmdline_runs_script(cl, "bot.py"));
        assert!(
            cmdline_runs_script(&cl.to_uppercase(), "bot.py"),
            "Windows 路径大小写不敏感"
        );
        assert!(cmdline_runs_script(
            "\"C:\\Python314\\python.exe\" D:\\x\\riskwatch.py",
            "riskwatch.py"
        ));
        assert!(!cmdline_runs_script(
            "\"C:\\Python314\\python.exe\" D:\\x\\replay.py",
            "bot.py"
        ));
        assert!(!cmdline_runs_script(
            "\"C:\\Python314\\python.exe\" D:\\x\\my_bot.py",
            "bot.py"
        ));
        assert!(!cmdline_runs_script(
            "\"C:\\Python314\\python.exe\" D:\\x\\bot.py.tmp",
            "bot.py"
        ));
        // 反斜杠、正斜杠、盘符根三种写法都认
        assert!(cmdline_runs_script("python /home/x/bot.py", "bot.py"));
        assert!(cmdline_runs_script("python D:bot.py", "bot.py"));
        // 只是名字里带 bot.py 的（前面不是路径分隔符）不算
        assert!(!cmdline_runs_script("python D:\\notes bot.py", "bot.py"));
        assert!(!cmdline_runs_script("python D:\\x\\notbot.py", "bot.py"));
    }

    #[test]
    fn same_python_accepts_pythonw_and_basename() {
        let cfg = "D:\\Python311\\python.exe";
        assert!(same_python("D:\\Python311\\python.exe", cfg));
        assert!(same_python("d:\\python311\\PYTHON.EXE", cfg), "盘符大小写");
        assert!(same_python("C:\\Other\\python.exe", cfg), "盘符换了仍认同一解释器名");
        assert!(same_python("D:\\Python311\\pythonw.exe", cfg), "pythonw 视同 python");
        assert!(!same_python("D:\\Python314\\python3.exe", "D:\\Python311\\python.exe"));
        assert!(!same_python("C:\\Windows\\notepad.exe", cfg));
        // pythonw 起 python 时启动参数里没有脚本路径，不会被当成 bot
        assert!(!cmdline_runs_script(
            "\"D:\\Python311\\pythonw.exe\" -c import runpy",
            "bot.py"
        ));
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

    /// 回归：读不存在的 PID 必须安静返回 None。
    /// 旧实现对远程 PEB 直接解引用，PID 恰好失效时把主进程打成 0xC0000005
    /// （2026-10-23 十个 dump 全停在 `read_unaligned(peb.add(0x20))`）。
    /// 这里用一个几乎不可能存活的号，走 OpenProcess 失败与句柄有效但远程读
    /// 失败两条分支都必须返回 None 的契约。
    #[cfg(windows)]
    #[test]
    fn process_command_line_of_dead_pid_returns_none() {
        // 4 是 System，映像名查询能过但 PEB 读会因缺权限失败——两条路都只能返回 None
        assert!(process_command_line(4).is_none());
        assert!(process_command_line(u32::MAX).is_none());
        // 当前进程自己：命令行读得到，且必须包含 cargo/test 相关字样
        let me = std::process::id();
        let cl = process_command_line(me).expect("自身命令行应可读");
        assert!(!cl.is_empty());
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
