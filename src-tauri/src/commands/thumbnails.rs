use tauri::{Emitter, Manager, State};

use crate::db;
use crate::models::Video;
use crate::scanner;

use super::AppState;

#[derive(Clone, serde::Serialize)]
pub struct ThumbnailProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    pub generated: usize,
    pub failed: usize,
}

/// 缩略图缓存目录：`<app_data>/thumbnails`
fn thumbnails_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("thumbnails");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建缩略图目录失败: {}", e))?;
    Ok(dir)
}

/// 为指定视频生成封面（ffmpeg 抽帧），已有有效缓存文件的会跳过。
/// 抽帧过程不持有数据库锁；每张成功后立即写库，中途关闭应用进度不丢。
#[tauri::command]
pub async fn generate_thumbnails(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    video_ids: Vec<String>,
) -> Result<usize, String> {
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法生成封面。请安装 ffmpeg 并加入 PATH。".into());
    }

    let dir = thumbnails_dir(&app)?;

    let jobs: Vec<Video> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let all = db::get_all_videos(&conn).map_err(|e| e.to_string())?;
        all.into_iter()
            .filter(|v| video_ids.iter().any(|id| id == &v.id))
            // 缓存文件仍在的跳过；文件被删掉的会重新生成
            .filter(|v| {
                !matches!(&v.thumbnail_path, Some(p) if std::path::Path::new(p).exists())
            })
            .collect()
    };

    let total = jobs.len();
    if total == 0 {
        let _ = app.emit(
            "thumbnail-progress",
            ThumbnailProgress { processed: 0, total: 0, done: true, generated: 0, failed: 0 },
        );
        return Ok(0);
    }

    let task_app = app.clone();
    let (generated, failed) = tauri::async_runtime::spawn_blocking(move || -> (usize, usize) {
        use tauri::Manager;
        let state = task_app.state::<AppState>();
        let mut generated = 0usize;
        let mut failed = 0usize;
        for (index, video) in jobs.iter().enumerate() {
            let out = dir.join(format!("{}.jpg", video.id));
            // 上次中途关闭留下的孤儿文件：直接登记，不重新抽帧
            let existing_ok = std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false);
            let ok = existing_ok || {
                // 抽帧写临时名，成功才改名——ffmpeg 中途被杀不会留下半张 jpg
                let part = dir.join(format!("{}.jpg.part", video.id));
                let result = scanner::extract_thumbnail(&video.path, &part, video.duration)
                    .and_then(|()| std::fs::rename(&part, &out).map_err(|e| e.to_string()));
                let _ = std::fs::remove_file(&part);
                result.is_ok()
            };
            if ok {
                if let Ok(conn) = state.db.lock() {
                    let _ = db::set_thumbnail(&conn, &video.id, &out.to_string_lossy());
                }
                generated += 1;
            } else {
                failed += 1;
            }
            let _ = task_app.emit(
                "thumbnail-progress",
                ThumbnailProgress {
                    processed: index + 1,
                    total,
                    done: false,
                    generated,
                    failed,
                },
            );
        }
        (generated, failed)
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "thumbnail-progress",
        ThumbnailProgress { processed: total, total, done: true, generated, failed },
    );

    Ok(generated)
}
