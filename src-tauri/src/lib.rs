mod commands;
mod db;
mod models;
mod potplayer;
mod scanner;
mod watcher;

use commands::AppState;
use db::init_db;
use tauri::Manager;

fn get_db_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录：{}", e))?;
    std::fs::create_dir_all(&app_dir)
        .map_err(|e| format!("无法创建应用数据目录 {}：{}", app_dir.display(), e))?;
    Ok(app_dir.join("viewman.db"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let db_path = get_db_path(app.handle()).map_err(|e| {
                eprintln!("viewman 启动失败：{}", e);
                tauri::Error::from(std::io::Error::other(e))
            })?;
            let conn = init_db(&db_path.to_string_lossy()).map_err(|e| {
                eprintln!("viewman 数据库初始化失败 ({}): {}", db_path.display(), e);
                tauri::Error::from(std::io::Error::other(format!("数据库初始化失败：{}", e)))
            })?;
            app.manage(AppState {
                db: std::sync::Mutex::new(conn),
                db_path: db_path.to_string_lossy().to_string(),
                cancel: std::sync::Mutex::new(std::collections::HashMap::new()),
                thumb_running: std::sync::Mutex::new(std::collections::HashSet::new()),
            });
            // 缓存目录迁移自愈：thumbnail_path 存的是绝对路径，cache_dir 改变后
            // 旧前缀全部失效。把仍指向旧 app_data 缩略图目录的记录改指当前缓存
            // 目录（纯前缀替换；前缀相同即零行更新，常开无害）
            {
                use tauri::Manager;
                let mig_handle = app.handle().clone();
                if let Ok(new_base) = commands::thumbnails_dir(&mig_handle) {
                    if let Ok(old_base) = app.path().app_data_dir() {
                        let old_prefix = old_base.join("thumbnails").to_string_lossy().to_string();
                        let new_prefix = new_base.to_string_lossy().to_string();
                        if old_prefix != new_prefix {
                            if let Ok(conn) = app.state::<AppState>().db.lock() {
                                for table in ["images", "videos"] {
                                    let _ = conn.execute(
                                        &format!(
                                            "UPDATE {table} SET thumbnail_path = ?1 || substr(thumbnail_path, ?2)                                              WHERE thumbnail_path LIKE ?3"
                                        ),
                                        rusqlite::params![
                                            format!("{}\\", new_prefix),
                                            old_prefix.chars().count() as i64 + 1,
                                            format!(r"{}\%", old_prefix),
                                        ],
                                    );
                                }
                            }
                        }
                    }
                }
            }
            // 扫描根目录监视：文件变化防抖后自动增量扫描（失败不影响手动扫描）
            watcher::spawn(app.handle().clone());
            // 后台清一轮孤儿缩略图：两张表都查无此 id 的缓存文件（失败不影响启动）
            let cleanup_app = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let removed = commands::cleanup_orphan_thumbnail_files(&cleanup_app);
                if removed > 0 {
                    println!("cleaned {} orphan thumbnail files", removed);
                }
                let removed = commands::cleanup_transcoded_files(&cleanup_app);
                if removed > 0 {
                    println!("cleaned {} orphan transcoded cache files", removed);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_videos,
            commands::get_video_view,
            commands::cancel_task,
            commands::scan_directory,
            commands::save_progress,
            commands::check_video_file,
            commands::get_playable_path,
            commands::convert_fake_images,
            commands::find_static_videos,
            commands::convert_short_videos,
            commands::find_hevc_videos,
            commands::convert_hevc_videos,
            commands::find_duplicate_videos,
            commands::get_videos_with_progress,
            commands::get_recently_played,
            commands::get_play_history,
            commands::check_ffprobe,
            commands::load_scan_roots,
            commands::save_scan_roots,
            commands::load_move_targets,
            commands::save_move_targets,
            commands::delete_videos,
            commands::move_video,
            commands::check_potplayer,
            commands::launch_potplayer,
            commands::potplayer_status,
            commands::generate_thumbnails,
            commands::resume_thumbnails,
            commands::get_images,
            commands::get_image_view,
            commands::get_image_stats,
            commands::get_missing_image_thumbnail_ids,
            commands::get_images_by_ids,
            commands::scan_image_directory,
            commands::delete_images,
            commands::move_image,
            commands::fix_mismatched_extensions,
            commands::generate_image_thumbnails,
            commands::find_duplicate_images,
            commands::find_images_like_template,
            commands::find_animated_images,
            commands::get_duplicate_cache,
            commands::get_video_duplicate_cache,
            commands::clear_detection_cache,
            commands::remove_media_directory,
            commands::capture_frame,
            commands::check_db_integrity,
            commands::rebuild_fts_indexes,
            commands::db_file_path,
            commands::load_bot_settings,
            commands::save_bot_settings,
            commands::get_bot_status,
            commands::start_bot,
            commands::stop_bot,
            commands::stop_all_bot,
            commands::get_napcat_status,
            commands::napcat_diagnose_cmd,
            commands::napcat_sync_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
