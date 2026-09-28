use crate::potplayer;
use crate::scanner;

/// 探测命令都在启动时被前端叫一次：里面要起子进程、查注册表，
/// 绝不能留在主线程上等，不然界面开场就是几秒冻结。
#[tauri::command]
pub async fn check_ffprobe() -> bool {
    tauri::async_runtime::spawn_blocking(|| {
        scanner::hidden_command("ffprobe")
            .arg("-version")
            .output()
            .is_ok()
    })
    .await
    .unwrap_or(false)
}

#[tauri::command]
pub async fn check_potplayer() -> bool {
    tauri::async_runtime::spawn_blocking(potplayer::find_potplayer)
        .await
        .ok()
        .flatten()
        .is_some()
}

#[tauri::command]
pub fn launch_potplayer(video_path: String, seek: Option<f64>) -> Result<(), String> {
    potplayer::launch(&video_path, seek)
}

#[tauri::command]
pub fn potplayer_status(video_path: String) -> potplayer::PotPlayerStatus {
    potplayer::get_status(&video_path)
}
