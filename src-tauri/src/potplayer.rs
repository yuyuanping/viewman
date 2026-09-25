use std::path::PathBuf;
use std::process::Command;

use crate::commands::MapErrStr;

static EXE_NAMES: &[&str] = &["PotPlayer.exe", "PotPlayerMini.exe", "PotPlayerMini64.exe"];
static INI_NAMES: &[&str] = &["PotPlayer.ini", "PotPlayerMini.ini"];

static BASE_DIRS: &[&str] = &[
    r"C:\Program Files\DAUM\PotPlayer",
    r"C:\Program Files (x86)\DAUM\PotPlayer",
    r"C:\Program Files\PotPlayer",
    r"C:\Program Files (x86)\PotPlayer",
    r"D:\PotPlayer",
    r"C:\PotPlayer",
];

pub fn find_potplayer() -> Option<PathBuf> {
    for dir in BASE_DIRS {
        for name in EXE_NAMES {
            let path = PathBuf::from(dir).join(name);
            if path.exists() {
                return Some(path);
            }
        }
    }
    find_via_registry()
}

fn find_via_registry() -> Option<PathBuf> {
    let keys = [
        (winreg::enums::HKEY_CURRENT_USER, r"SOFTWARE\DAUM\PotPlayer"),
        (winreg::enums::HKEY_LOCAL_MACHINE, r"SOFTWARE\DAUM\PotPlayer"),
        (winreg::enums::HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\DAUM\PotPlayer"),
    ];
    for &(hkey, subkey) in &keys {
        if let Ok(hk) = winreg::RegKey::predef(hkey).open_subkey_with_flags(subkey, winreg::enums::KEY_READ) {
            if let Ok(path_str) = hk.get_value::<String, _>("InstallPath") {
                let p = PathBuf::from(&path_str);
                if p.join("PotPlayer.exe").exists() {
                    return Some(p.join("PotPlayer.exe"));
                }
            }
        }
    }
    None
}

pub fn launch(path: &str, seek: Option<f64>) -> Result<(), String> {
    let exe = find_potplayer().ok_or_else(|| "PotPlayer not found")?;
    let mut cmd = Command::new(&exe);
    cmd.arg(path);
    if let Some(s) = seek {
        if s > 0.0 {
            cmd.arg(format!("/seek={}", s));
        }
    }
    cmd.spawn().map_err(|e| format!("Failed to launch PotPlayer: {}", e))?;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct PotPlayerStatus {
    pub running: bool,
    /// "running"（找到本文件的窗口）| "unknown"（有 PotPlayer 窗口但不匹配本文件）| "stopped"（没有任何 PotPlayer 窗口，可确认播放器已退出）
    pub state: &'static str,
    pub position: Option<f64>,
    /// 目前仅可能是 "live"（来自窗口标题），不再回传 ini 记忆值
    pub position_source: Option<&'static str>,
}

pub fn get_status(video_path: &str) -> PotPlayerStatus {
    let filename = std::path::Path::new(video_path)
        .file_name()
        .and_then(|n| n.to_str());
    let matched = filename.and_then(find_potplayer_window);

    if let Some(title) = matched {
        let position = parse_time_from_title(&title);
        PotPlayerStatus {
            running: true,
            state: "running",
            position,
            position_source: position.map(|_| "live"),
        }
    } else if potplayer_window_exists() {
        PotPlayerStatus { running: false, state: "unknown", position: None, position_source: None }
    } else {
        PotPlayerStatus { running: false, state: "stopped", position: None, position_source: None }
    }
}

pub fn enable_titlebar_time() -> Result<(), String> {
    let ini_path = find_or_create_ini()?;
    let content = std::fs::read_to_string(&ini_path).map_err_str()?;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    const TARGET_KEY: &str = "ShowCurrentTimeInTitle";

    let mut osd_start: Option<usize> = None;
    let mut osd_end: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("[osd]") {
            osd_start = Some(i);
            continue;
        }
        if osd_start.is_some() && osd_end.is_none() {
            if trimmed.starts_with('[') {
                osd_end = Some(i);
            }
        }
    }

    if let Some(start) = osd_start {
        let end = osd_end.unwrap_or(lines.len());
        for i in start + 1..end {
            let trimmed = lines[i].trim();
            let lower = trimmed.to_lowercase();
            if lower.contains(TARGET_KEY.to_lowercase().as_str()) {
                let parts: Vec<&str> = trimmed.splitn(2, '=').collect();
                if parts.len() == 2 && parts[1].trim() != "1" {
                    lines[i] = format!("{}={}", parts[0], 1);
                    std::fs::write(&ini_path, lines.join("\r\n")).map_err_str()?;
                    return Ok(());
                }
                return Ok(());
            }
        }
        lines.insert(end, format!("{}={}", TARGET_KEY, 1));
    } else {
        lines.push(String::new());
        lines.push(format!("[{}]", "OSD"));
        lines.push(format!("{}={}", TARGET_KEY, 1));
    }

    std::fs::write(&ini_path, lines.join("\r\n")).map_err_str()
}

fn find_or_create_ini() -> Result<PathBuf, String> {
    if let Some(p) = find_ini_file() {
        return Ok(p);
    }

    let (dir, ini_name) = if let Some(exe) = find_potplayer() {
        let parent = exe.parent().ok_or_else(|| "no parent dir".to_string())?;
        let name = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ini_name = if name.starts_with("PotPlayerMini") { "PotPlayerMini.ini" } else { "PotPlayer.ini" };
        (parent.to_path_buf(), ini_name.to_string())
    } else {
        let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA not found".to_string())?;
        let d = PathBuf::from(&appdata).join("PotPlayer");
        std::fs::create_dir_all(&d).map_err_str()?;
        (d, "PotPlayerMini.ini".to_string())
    };

    let path = dir.join(&ini_name);
    std::fs::write(&path, "").map_err_str()?;
    Ok(path)
}

fn find_ini_file() -> Option<PathBuf> {
    if let Some(exe_path) = find_potplayer() {
        let dir = exe_path.parent()?;
        for name in INI_NAMES {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }

    let appdata = std::env::var("APPDATA").ok()?;
    for name in INI_NAMES {
        let p = PathBuf::from(&appdata).join("PotPlayer").join(name);
        if p.exists() {
            return Some(p);
        }
    }

    let local_appdata = std::env::var("LOCALAPPDATA").ok()?;
    for name in INI_NAMES {
        let p = PathBuf::from(&local_appdata).join("PotPlayer").join(name);
        if p.exists() {
            return Some(p);
        }
    }

    None
}

type HWND = *mut std::ffi::c_void;
type BOOL = i32;
type LPARAM = isize;

const TRUE: BOOL = 1;
const FALSE: BOOL = 0;

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(lpEnumFunc: Option<unsafe extern "system" fn(HWND, LPARAM) -> BOOL>, lParam: LPARAM) -> BOOL;
    fn GetWindowTextW(hWnd: HWND, lpString: *mut u16, nMaxCount: i32) -> i32;
    fn IsWindowVisible(hWnd: HWND) -> BOOL;
}

struct EnumCtx {
    filename: String,
    result: Option<String>,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam as *mut EnumCtx);
    if IsWindowVisible(hwnd) != TRUE {
        return TRUE;
    }
    let mut buf = [0u16; 1024];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), 1024);
    if len <= 0 {
        return TRUE;
    }
    let title = String::from_utf16_lossy(&buf[..len as usize]);
    if !title.contains("PotPlayer") {
        return TRUE;
    }
    if !title.contains(&ctx.filename) {
        return TRUE;
    }
    ctx.result = Some(title);
    FALSE
}

fn find_potplayer_window(filename: &str) -> Option<String> {
    let mut ctx = EnumCtx {
        filename: filename.to_string(),
        result: None,
    };
    unsafe {
        EnumWindows(
            Some(enum_proc),
            &mut ctx as *mut EnumCtx as LPARAM,
        );
    }
    ctx.result
}

/// 只要存在任何可见的 PotPlayer 窗口即返回 true（不区分文件）
fn potplayer_window_exists() -> bool {
    let mut found = false;
    unsafe {
        EnumWindows(Some(enum_potplayer_proc), &mut found as *mut bool as LPARAM);
    }
    found
}

unsafe extern "system" fn enum_potplayer_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let found = &mut *(lparam as *mut bool);
    if IsWindowVisible(hwnd) != TRUE {
        return TRUE;
    }
    let mut buf = [0u16; 256];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), 256);
    if len > 0 {
        let title = String::from_utf16_lossy(&buf[..len as usize]);
        if title.contains("PotPlayer") {
            *found = true;
            return FALSE;
        }
    }
    TRUE
}

fn parse_time_from_title(title: &str) -> Option<f64> {
    let bytes = title.as_bytes();
    let len = bytes.len();

    // 第一种模式（H:MM:SS）占用 i..=i+6，第二种（HH:MM:SS）自带 i+7<len 校验
    for i in 0..len.saturating_sub(6) {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1] == b':'
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && bytes[i + 4] == b':'
            && bytes[i + 5].is_ascii_digit()
            && bytes[i + 6].is_ascii_digit()
        {
            let h: u32 = (bytes[i] - b'0') as u32;
            let m: u32 = ((bytes[i + 2] - b'0') as u32) * 10 + ((bytes[i + 3] - b'0') as u32);
            let s: u32 = ((bytes[i + 5] - b'0') as u32) * 10 + ((bytes[i + 6] - b'0') as u32);

            if m < 60 && s < 60 {
                return Some(h as f64 * 3600.0 + m as f64 * 60.0 + s as f64);
            }
        }

        if i + 7 < len
            && bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2] == b':'
            && bytes[i + 3].is_ascii_digit()
            && bytes[i + 4].is_ascii_digit()
            && bytes[i + 5] == b':'
            && bytes[i + 6].is_ascii_digit()
            && bytes[i + 7].is_ascii_digit()
        {
            let h: u32 = ((bytes[i] - b'0') as u32) * 10 + ((bytes[i + 1] - b'0') as u32);
            let m: u32 = ((bytes[i + 3] - b'0') as u32) * 10 + ((bytes[i + 4] - b'0') as u32);
            let s: u32 = ((bytes[i + 6] - b'0') as u32) * 10 + ((bytes[i + 7] - b'0') as u32);

            if m < 60 && s < 60 {
                return Some(h as f64 * 3600.0 + m as f64 * 60.0 + s as f64);
            }
        }
    }
    None
}
