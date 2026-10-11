//! NapCat 管理层：把 NapCat 的运行状态从"对着日志猜"变成结构化数据。
//!
//! 背景（都是实测踩过的坑，不是假想）：
//! - NapCat 是注入 QQ.exe 的独立进程（Node 内核 + DLL hook），Viewman 只能管理、不能嵌入。
//! - 它**顺着 QQ.exe 的路径**去找配置目录。本机装了两套 Shell（完整核心 4.18.28 +
//!   只有 QQ.exe 的 44498 壳），因此核心会读到 44498 下那份空的 onebot11 配置，
//!   表现为"适配器初始化完成但端口不开"。napcat_diagnose 专门查这种配置漂移。
//! - 每次重启都要扫码，登录前端口不会监听；二维码约 2 分钟过期后自动重出。
//!   所以"没反应"绝大多数时候是在等扫码，必须让面板说清楚这一点。
//!
//! 数据来源全部是本地真实文件：console 日志（start.bat 重定向）、二维码图片、
//! 两套 Shell 的 config 目录。不依赖网络，也不改 NapCat 自身。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{silent_command, MapErrStr};

// start.bat 把 NapCat 的 stdout 重定向到这个文件（见 qqbot/start.bat）
const CONSOLE_LOG: &str = "D:\\QQBot_Data\\napcat_console.log";
// OneKey 套件根目录：里面有多套 Shell，靠扫描发现，不写死具体版本号
const NAPCAT_ROOT: &str = "D:\\tool\\NapCat.Shell.Windows.OneKey";
const QQ_EXE_NAME: &str = "QQ.exe";

/// NapCat 的登录/启动阶段。面板据此显示"现在到底卡在哪一步"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NapCatStage {
    /// 没找到 NapCat 进程，也没看到相关日志
    NotRunning,
    /// 进程在，日志还没进入登录阶段
    Starting,
    /// 起来了，卡在等扫码（这是最常见的"点了没反应"）
    WaitingQr,
    /// 扫码成功、适配器已初始化，正在等端口起来
    Initializing,
    /// WS + HTTP 端口都已监听，可以对接
    Ready,
    /// 日志里有明确的错误
    Error,
}

/// 一条 NapCat 状态快照：阶段 + 给人看的结论 + 可执行的下一步。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NapCatStatus {
    pub stage: NapCatStage,
    /// NapCatWinBootMain.exe 的 PID（None = 没在跑）
    pub pid: Option<u32>,
    /// 当前要登录的 QQ 号（日志"正在快速登录 xxx"）
    pub account: Option<String>,
    /// 二维码图片路径；处于 WaitingQr 时必定有，面板直接显示
    pub qr_image: Option<String>,
    /// 面向用户的结论，一句话说清现在是什么情况
    pub summary: String,
    /// 内核版本（"NapCat.Core Version: x.y.z"）
    pub core_version: Option<String>,
    /// WebUI 地址，方便用户直接点开改配置
    pub webui_url: Option<String>,
}

/// 配置漂移体检结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NapCatDiagnose {
    /// 发现了几套 Shell
    pub shells: Vec<String>,
    /// 核心所在目录（含 napcat.mjs 的那套）
    pub core_dir: Option<String>,
    /// QQ.exe 所在目录
    pub qq_dir: Option<String>,
    /// NapCat 实际会去读的配置目录（跟着 QQ.exe 走）
    pub effective_config_dir: Option<String>,
    /// 每个账号配置文件里启用的端口，key 是文件名
    pub config_ports: Vec<NapCatConfigEntry>,
    /// 检测到的问题（配置漂移、端口被禁用、目录缺失……）
    pub issues: Vec<String>,
    /// 一切正常时为 true
    pub healthy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NapCatConfigEntry {
    pub file: String,
    pub path: String,
    /// 账号（从 onebot11_<uin>.json 的文件名解析）
    pub account: Option<String>,
    pub ws_ports: Vec<u16>,
    pub http_ports: Vec<u16>,
    pub ws_enabled: bool,
    pub http_enabled: bool,
}

/// 解析单行日志 → 事件。日志格式形如：
/// `10-10 21:46:31 [warn] 二维码已保存到 D:\...\qrcode.png`
/// `10-10 21:38:07 [info] [AdapterManager] 协议适配器初始化完成，已加载 2 个适配器`
#[derive(Debug, Clone, PartialEq)]
pub enum LogEvent {
    /// 二维码已生成，附带保存路径
    QrSaved(String),
    /// 二维码过期/登录失败，NapCat 会重新出一张
    LoginError(String),
    /// 正在快速登录某个账号
    QuickLogin(String),
    /// 适配器初始化完成
    AdapterReady,
    /// 内核版本
    CoreVersion(String),
    /// WebUI 地址
    WebUiUrl(String),
    /// 其它 error/warn，按级别记下来给用户看
    Diagnostic(String),
}

/// 从一行日志解析出事件。抽成纯函数是为了能单测。
pub fn parse_log_line(line: &str) -> Option<LogEvent> {
    let line = strip_ansi(line);
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // 只认带时间戳的日志行，二维码点阵那些行没有时间戳，会被这里挡掉
    let is_timed = {
        let b = line.as_bytes();
        // "MM-DD HH:MM:SS "
        b.len() > 14 && b[2] == b'-' && b[5] == b' ' && b[8] == b':'
    };
    if !is_timed {
        return None;
    }
    if line.contains("二维码已保存到") {
        let path = line
            .split_once("二维码已保存到")
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or_default();
        if !path.is_empty() {
            return Some(LogEvent::QrSaved(path));
        }
    }
    if line.contains("[Core] [Login] Login Error") {
        return Some(LogEvent::LoginError(line.to_string()));
    }
    if let Some(rest) = line.split_once("正在快速登录") {
        let acct = rest
            .1
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if !acct.is_empty() {
            return Some(LogEvent::QuickLogin(acct));
        }
    }
    if line.contains("协议适配器初始化完成") {
        return Some(LogEvent::AdapterReady);
    }
    if let Some(rest) = line.split_once("NapCat.Core Version:") {
        return Some(LogEvent::CoreVersion(rest.1.trim().to_string()));
    }
    if line.contains("WebUi User Panel Url:") {
        if let Some((_, rest)) = line.split_once("WebUi User Panel Url:") {
            return Some(LogEvent::WebUiUrl(rest.trim().to_string()));
        }
    }
    // 二维码提示文案是正常流程的噪声，不当错误上报（否则每次等扫码面板都一堆假告警）
    if line.contains("请扫描下面的二维码")
        || line.contains("请扫描下面的二维码，然后在手Q上授权登录")
    {
        return None;
    }
    let is_err = line.contains("[error]") || line.contains("[fatal]");
    let is_warn = line.contains("[warn]");
    if is_err || is_warn {
        return Some(LogEvent::Diagnostic(line.to_string()));
    }
    None
}

/// 去掉 ANSI 颜色转义（NapCat 的日志带色，直接读会有乱码）
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ ... 字母  —— 跳过 CSI 序列
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 把 console 日志整体折叠成状态：取最后若干条相关事件，按"最新胜出"。
/// 单独抽出来是为了能对着真实日志文本做单测。
pub fn summarize_log(text: &str, ports_up: bool) -> NapCatStatus {
    let mut account = None;
    let mut qr_image: Option<String> = None;
    let mut core_version = None;
    let mut webui_url = None;
    let mut adapter_ready = false;
    let mut login_error: Option<String> = None;
    let mut login_error_at = String::new();
    let mut last_ts = String::new();

    for line in text.lines() {
        let clean = strip_ansi(line);
        if let Some(ts) = timestamp_of(&clean) {
            last_ts = ts;
        }
        match parse_log_line(&clean) {
            Some(LogEvent::QrSaved(p)) => {
                qr_image = Some(p);
                login_error = None;
            }
            Some(LogEvent::LoginError(msg)) => {
                login_error = Some(msg);
                login_error_at = last_ts.clone();
            }
            Some(LogEvent::QuickLogin(a)) => account = Some(a),
            Some(LogEvent::AdapterReady) => adapter_ready = true,
            Some(LogEvent::CoreVersion(v)) => core_version = Some(v),
            Some(LogEvent::WebUiUrl(_)) => {
                if let Some((_, rest)) = line.split_once("WebUi User Panel Url:") {
                    webui_url = Some(rest.trim().to_string());
                }
            }
            // 诊断性 warn/error 行不进状态快照（最近错误功能已移除）
            Some(LogEvent::Diagnostic(_)) => {}
            None => {}
        }
    }

    let stage = if let Some(err) = &login_error {
        // 登录失败且还没出新码 = 码过期，等用户重扫
        let _ = err;
        NapCatStage::WaitingQr
    } else if qr_image.is_some() && !adapter_ready {
        NapCatStage::WaitingQr
    } else if ports_up && adapter_ready {
        NapCatStage::Ready
    } else if adapter_ready {
        NapCatStage::Initializing
    } else if !last_ts.is_empty() {
        NapCatStage::Starting
    } else {
        NapCatStage::NotRunning
    };

    let summary = build_summary(
        stage,
        &login_error,
        &login_error_at,
        ports_up,
        adapter_ready,
    );

    NapCatStatus {
        stage,
        pid: None, // 由调用方用进程探测补上
        account,
        qr_image,
        summary,
        core_version,
        webui_url,
    }
}

fn build_summary(
    stage: NapCatStage,
    login_error: &Option<String>,
    _login_error_at: &str,
    ports_up: bool,
    adapter_ready: bool,
) -> String {
    match stage {
        NapCatStage::NotRunning => "NapCat 未运行".to_string(),
        NapCatStage::Starting => "NapCat 正在启动".to_string(),
        NapCatStage::WaitingQr => match login_error {
            Some(_) => {
                "二维码已过期，请在面板里重新扫码（NapCat 每隔约 2 分钟会自动换一张）".to_string()
            }
            None => "等待扫码登录：请用手机 QQ 扫描面板里的二维码并确认".to_string(),
        },
        NapCatStage::Initializing => {
            if adapter_ready && !ports_up {
                "适配器已加载但端口仍未监听，多半是 onebot11 配置里没有启用服务器".to_string()
            } else {
                "正在初始化 OneBot 适配器".to_string()
            }
        }
        NapCatStage::Ready => "OneBot11 已就绪（WS/HTTP 端口均已监听）".to_string(),
        NapCatStage::Error => "NapCat 报错，请看下方错误列表".to_string(),
    }
}

/// 取出 "MM-DD HH:MM:SS" 前缀，用来比较新旧（够用了，不跨年）
fn timestamp_of(line: &str) -> Option<String> {
    let b = line.as_bytes();
    if b.len() < 14 || b[2] != b'-' || b[5] != b' ' || b[8] != b':' {
        return None;
    }
    Some(line[..14].trim().to_string())
}

/// 找 NapCatWinBootMain.exe 的 PID
pub fn find_napcat_pid() -> Option<u32> {
    let script = "Get-CimInstance Win32_Process -Filter \"Name='NapCatWinBootMain.exe'\" | ForEach-Object { $_.ProcessId }";
    let out = silent_command("powershell")
        .args(["-NoProfile", "-Command", script])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.trim().parse::<u32>().ok())
}

fn read_to_string(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// 端口是否在监听（复用 qqbot 的探测思路）
fn ports_up(ws: u16, http: u16) -> (bool, bool) {
    let out = silent_command("netstat")
        .args(["-ano", "-p", "tcp"])
        .output();
    let listening = match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return (false, false),
    };
    let has = |port: u16| {
        listening
            .lines()
            .filter(|l| l.contains("LISTENING"))
            .any(|l| {
                l.split_whitespace()
                    .any(|f| f.ends_with(&format!(":{}", port)))
            })
    };
    (has(ws), has(http))
}

/// 扫描 OneKey 根目录下所有 Shell（名字含 .Shell 的目录）
fn discover_shells() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(NAPCAT_ROOT) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.contains(".Shell") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// 解析一份 onebot11_*.json，取出 WS/HTTP 端口与启用状态
fn parse_onebot11(path: &Path) -> Option<NapCatConfigEntry> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let net = v.get("network")?;
    let collect = |key: &str| -> (Vec<u16>, bool) {
        let mut ports = Vec::new();
        let mut any_enabled = false;
        if let Some(arr) = net.get(key).and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(p) = item.get("port").and_then(|x| x.as_u64()) {
                    ports.push(p as u16);
                }
                if item.get("enable").and_then(|x| x.as_bool()) == Some(true) {
                    any_enabled = true;
                }
            }
        }
        (ports, any_enabled)
    };
    let (ws_ports, ws_enabled) = collect("websocketServers");
    let (http_ports, http_enabled) = collect("httpServers");
    let file = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let account = file
        .strip_prefix("onebot11_")
        .and_then(|s| s.strip_suffix(".json"))
        .map(|s| s.to_string());
    Some(NapCatConfigEntry {
        file,
        path: path.to_string_lossy().to_string(),
        account,
        ws_ports,
        http_ports,
        ws_enabled,
        http_enabled,
    })
}

/// 在一个 Shell 目录里递归找 napcat.mjs（限深，跳过 node_modules）。
/// OneKey 的 4.18.x 是平铺布局（napcat.mjs 就在根），但 44498 这种自包含版
/// 藏在 versions\<ver>\resources\app\napcat\ 下，只查根会误报「没找到内核」。
fn find_core_dir(shell: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: usize) -> Option<PathBuf> {
        if dir.join("napcat.mjs").is_file() {
            return Some(dir.to_path_buf());
        }
        if depth == 0 {
            return None;
        }
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // node_modules 很大且不含内核；logs/cache 也没必要进
            if name == "node_modules" || name == "logs" || name == "cache" {
                continue;
            }
            if let Some(hit) = walk(&p, depth - 1) {
                return Some(hit);
            }
        }
        None
    }
    // shell/versions/<ver>/resources/app/napcat/napcat.mjs = 4 层，5 层留余量
    walk(shell, 5)
}

/// 配置体检：找出配置漂移、空配置、版本错配等问题。
pub fn napcat_diagnose() -> NapCatDiagnose {
    let shells = discover_shells();
    let mut issues = Vec::new();

    // 含 napcat.mjs 的目录 = 真正的内核目录（可能深嵌在 versions\ 下）
    let core_dir = shells.iter().find_map(|p| find_core_dir(p));
    // 含 QQ.exe 的目录 = NapCat 真正跟随的配置来源
    let qq_dir = shells
        .iter()
        .find(|p| p.join(QQ_EXE_NAME).is_file())
        .cloned();

    if shells.is_empty() {
        issues.push(format!("在 {} 下没找到任何 NapCat Shell 目录", NAPCAT_ROOT));
    }
    if core_dir.is_none() {
        issues.push("没找到含 napcat.mjs 的核心目录".to_string());
    }
    if qq_dir.is_none() {
        issues.push("没找到 QQ.exe，NapCat 无法启动".to_string());
    }

    // NapCat 跟着 QQ.exe 找配置：<QQ目录>\versions\<版本>\resources\app\napcat\config
    let effective_config_dir = qq_dir.as_ref().and_then(|q| {
        let versions = q.join("versions");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&versions)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs.last().map(|v| {
            v.join("resources")
                .join("app")
                .join("napcat")
                .join("config")
        })
    });

    let mut config_ports = Vec::new();
    if let Some(cfg_dir) = &effective_config_dir {
        if !cfg_dir.is_dir() {
            issues.push(format!("生效配置目录不存在: {}", cfg_dir.display()));
        } else {
            let mut files: Vec<PathBuf> = std::fs::read_dir(cfg_dir)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .map(|n| n.starts_with("onebot11"))
                                .unwrap_or(false)
                        })
                        .collect()
                })
                .unwrap_or_default();
            files.sort();
            for f in files {
                if let Some(entry) = parse_onebot11(&f) {
                    if !entry.ws_enabled && !entry.http_enabled {
                        issues.push(format!(
                            "{} 里没有启用任何服务器——这正是“适配器就绪但端口不开”的原因",
                            entry.file
                        ));
                    }
                    config_ports.push(entry);
                }
            }
            if config_ports.is_empty() {
                issues.push(format!(
                    "生效配置目录里没有 onebot11 配置: {}",
                    cfg_dir.display()
                ));
            }
        }
    }

    // 配置漂移的真信号：内核目录与 QQ.exe 是否属于同一个 Shell 目录。
    // 跨 Shell 时 NapCat 会顺着 QQ.exe 去另一套读配置——今晚的故障正是
    // NAPCAT_DIR=4.18.28 + NAPCAT_QQ=44498 这个交叉接线造成的。
    // 注意：不能用 package.json 的 version 比对——它是 npm 占位符 0.0.1，
    // 而 Shell 名里的 44498 是 QQ build 号，两者是不同编号体系，永远不等。
    let shell_of = |p: &Path| -> Option<String> {
        p.ancestors()
            .find(|a| {
                a.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(".Shell"))
                    .unwrap_or(false)
            })
            .map(|a| a.to_string_lossy().to_string())
    };
    if let (Some(cd), Some(qd)) = (&core_dir, &qq_dir) {
        let core_shell = shell_of(cd);
        let qq_shell = shell_of(qd);
        if let (Some(cs), Some(qs)) = (&core_shell, &qq_shell) {
            if cs != qs {
                issues.push(format!(
                    "内核目录({})与 QQ.exe({})分属两套 Shell，NapCat 会跨目录读配置、容易错配",
                    cs, qs
                ));
            }
        }
    }
    // 内核版本仅供参考（package.json 里的 version 是 npm 占位符，真实版本见状态面板的日志解析）
    let _core_ver_hint = core_dir.as_ref().and_then(|d| {
        std::fs::read_to_string(d.join("package.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(String::from))
    });

    let healthy = issues.is_empty();
    NapCatDiagnose {
        shells: shells
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        core_dir: core_dir.map(|p| p.to_string_lossy().to_string()),
        qq_dir: qq_dir.map(|p| p.to_string_lossy().to_string()),
        effective_config_dir: effective_config_dir.map(|p| p.to_string_lossy().to_string()),
        config_ports,
        issues,
        healthy,
    }
}

/// Tauri 命令：取 NapCat 状态快照
#[tauri::command]
pub async fn get_napcat_status() -> Result<NapCatStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let text = read_to_string(CONSOLE_LOG).unwrap_or_default();
        let (ws_up, http_up) = ports_up(9501, 9502);
        let mut status = summarize_log(&text, ws_up && http_up);
        status.pid = find_napcat_pid();
        if status.pid.is_none() && status.stage != NapCatStage::NotRunning {
            // 进程没了但日志还在，收敛回"未运行"，别让面板一直显示等待扫码
            status.stage = NapCatStage::NotRunning;
            status.summary = "NapCat 未运行（检测到历史日志，但进程已退出）".to_string();
            status.qr_image = None;
        }
        Ok(status)
    })
    .await
    .map_err_str()?
}

/// Tauri 命令：配置体检
#[tauri::command]
pub async fn napcat_diagnose_cmd() -> Result<NapCatDiagnose, String> {
    tauri::async_runtime::spawn_blocking(napcat_diagnose)
        .await
        .map_err_str()
}

/// Tauri 命令：把 core 目录下的 onebot11 配置同步到生效配置目录。
/// 这就是今晚手工做的修复（NapCat 跨目录读到了空配置），做成可一键执行。
#[tauri::command]
pub async fn napcat_sync_config() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<String, String> {
        let diag = napcat_diagnose();
        let core = diag
            .core_dir
            .ok_or_else(|| "找不到 NapCat 核心目录（应含 napcat.mjs）".to_string())?;
        let eff = diag
            .effective_config_dir
            .ok_or_else(|| "找不到生效配置目录（应在 QQ.exe 的 versions 下）".to_string())?;
        if !Path::new(&eff).is_dir() {
            std::fs::create_dir_all(&eff).map_err_str()?;
        }
        let src = Path::new(&core).join("config");
        let mut copied = Vec::new();
        let rd = std::fs::read_dir(&src).map_err_str()?;
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("onebot11") || !p.is_file() {
                continue;
            }
            std::fs::copy(&p, Path::new(&eff).join(name)).map_err_str()?;
            copied.push(name.to_string());
        }
        if copied.is_empty() {
            return Err(format!(
                "核心目录 {} 下没有 onebot11 配置可同步",
                src.display()
            ));
        }
        copied.sort();
        Ok(format!(
            "已同步 {} 份配置到生效目录：{}。重启 NapCat 后生效。",
            copied.len(),
            copied.join("、")
        ))
    })
    .await
    .map_err_str()?
}

#[cfg(test)]
mod tests {
    use super::*;

    // 占位数据：token 与 QQ 号均为虚构，不要填入真实凭据/个人信息
    const SAMPLE: &str = "\
10-10 21:46:30 [info] [NapCat] [Core] NapCat.Core Version: 4.18.28
10-10 21:46:30 [info] [NapCat] [WebUi] WebUi User Panel Url: http://127.0.0.1:6099/webui?token=xxxxxx
10-10 21:46:31 [info] 没有 -q 指令指定快速登录，将使用二维码登录方式
10-10 21:46:31 [info] 正在快速登录  123456789
10-10 21:46:31 [warn] 请扫描下面的二维码，然后在手Q上授权登录：
10-10 21:46:31 [warn] 二维码已保存到 D:\\QQBot_Data\\cache\\qrcode.png
";

    #[test]
    fn ansi_is_stripped() {
        let line = "10-10 21:46:31 [\u{1b}[33mwarn\u{1b}[0m] 二维码已保存到 D:\\x\\qrcode.png";
        assert_eq!(
            parse_log_line(line),
            Some(LogEvent::QrSaved("D:\\x\\qrcode.png".to_string()))
        );
    }

    #[test]
    fn qr_ascii_art_is_ignored() {
        // 二维码点阵没有时间戳，必须被忽略，否则会污染状态
        let art = "█ ▄▄▄ █▄▀ ▄▄ █▀ ▄▄▀▄▄▀█▄▀█ ▀▀";
        assert_eq!(parse_log_line(art), None);
    }

    #[test]
    fn core_dir_found_when_nested_under_versions() {
        // 44498 布局：napcat.mjs 在 versions\<ver>\resources\app\napcat\ 下，
        // 只查 shell 根目录会误报「没找到内核」
        let tmp = std::env::temp_dir().join(format!("vm_nc_{}", std::process::id()));
        let core = tmp
            .join("NapCat.44498.Shell")
            .join("versions")
            .join("9.9.26-44498")
            .join("resources")
            .join("app")
            .join("napcat");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::write(core.join("napcat.mjs"), "").unwrap();
        // 顺便确认 node_modules 不会被遍历
        std::fs::create_dir_all(tmp.join("NapCat.44498.Shell/node_modules")).unwrap();
        std::fs::write(tmp.join("NapCat.44498.Shell/node_modules/napcat.mjs"), "").unwrap();

        let found = find_core_dir(&tmp.join("NapCat.44498.Shell"));
        assert_eq!(found.as_deref(), Some(core.as_path()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn waiting_qr_when_no_login_yet() {
        let s = summarize_log(SAMPLE, false);
        assert_eq!(s.stage, NapCatStage::WaitingQr);
        assert_eq!(s.account.as_deref(), Some("123456789"));
        assert_eq!(
            s.qr_image.as_deref(),
            Some("D:\\QQBot_Data\\cache\\qrcode.png")
        );
        assert_eq!(s.core_version.as_deref(), Some("4.18.28"));
        assert!(s.webui_url.is_some());
    }

    #[test]
    fn expired_qr_stays_waiting_and_reports_expiry() {
        let log = format!(
            "{}\n10-10 21:48:32 [error] [Core] [Login] Login Error,ErrType:  1  ErrCode: 3\n",
            SAMPLE
        );
        let s = summarize_log(&log, false);
        assert_eq!(s.stage, NapCatStage::WaitingQr);
        assert!(s.summary.contains("过期"), "summary = {}", s.summary);
    }

    #[test]
    fn ready_requires_both_adapter_and_ports() {
        let log = format!(
            "{}10-10 21:38:07 [info] [AdapterManager] 协议适配器初始化完成，已加载 2 个适配器\n",
            SAMPLE
        );
        // 适配器就绪但端口没开 -> Initializing，且要说清是配置问题
        let s = summarize_log(&log, false);
        assert_eq!(s.stage, NapCatStage::Initializing);
        assert!(s.summary.contains("配置"), "summary = {}", s.summary);
        // 端口也开了 -> Ready
        let s2 = summarize_log(&log, true);
        assert_eq!(s2.stage, NapCatStage::Ready);
    }

    #[test]
    fn empty_log_is_not_running() {
        let s = summarize_log("", false);
        assert_eq!(s.stage, NapCatStage::NotRunning);
    }
}
