use std::path::PathBuf;
use std::process::Command;

static EXE_NAMES: &[&str] = &["PotPlayer.exe", "PotPlayerMini.exe", "PotPlayerMini64.exe"];

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
    let exe = find_potplayer().ok_or("PotPlayer not found")?;
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

#[allow(clippy::upper_case_acronyms)] // Win32 API 惯例名
type HWND = *mut std::ffi::c_void;
#[allow(clippy::upper_case_acronyms)] // Win32 API 惯例名
type BOOL = i32;
#[allow(clippy::upper_case_acronyms)] // Win32 API 惯例名
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
