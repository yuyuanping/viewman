mod commands;
mod db;
mod models;
mod potplayer;
mod scanner;

use commands::AppState;
use db::init_db;
use tauri::Manager;

fn get_db_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    let app_dir = app.path().app_data_dir().expect("failed to get app data dir");
    std::fs::create_dir_all(&app_dir).expect("failed to create app data dir");
    app_dir.join("viewman.db")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let db_path = get_db_path(&app.handle());
            let conn = init_db(&db_path.to_string_lossy())
                .expect("failed to initialize database");
            app.manage(AppState {
                db: std::sync::Mutex::new(conn),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_videos,
            commands::scan_directory,
            commands::save_progress,
            commands::check_video_file,
            commands::get_progress,
            commands::get_videos_with_progress,
            commands::get_recently_played,
            commands::check_ffprobe,
            commands::delete_video,
            commands::check_potplayer,
            commands::launch_potplayer,
            commands::potplayer_status,
            commands::enable_potplayer_titlebar,
            commands::generate_thumbnails,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
