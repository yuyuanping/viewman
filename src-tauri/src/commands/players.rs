use crate::potplayer;
use crate::scanner;

#[tauri::command]
pub fn check_ffprobe() -> bool {
    scanner::hidden_command("ffprobe")
        .arg("-version")
        .output()
        .is_ok()
}

#[tauri::command]
pub fn check_potplayer() -> bool {
    potplayer::find_potplayer().is_some()
}

#[tauri::command]
pub fn launch_potplayer(video_path: String, seek: Option<f64>) -> Result<(), String> {
    potplayer::launch(&video_path, seek)
}

#[tauri::command]
pub fn enable_potplayer_titlebar() -> Result<(), String> {
    potplayer::enable_titlebar_time()
}

#[tauri::command]
pub fn potplayer_status(video_path: String) -> potplayer::PotPlayerStatus {
    potplayer::get_status(&video_path)
}
