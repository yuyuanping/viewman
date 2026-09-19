use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, State};

use crate::db;
use crate::models::Video;
use crate::scanner;

use super::AppState;

#[derive(Clone, serde::Serialize)]
pub struct ScanProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    pub warnings: Vec<String>,
}

#[tauri::command]
pub async fn scan_directory(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<Vec<Video>, String> {
    let existing_videos = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_all_videos(&conn).map_err(|e| e.to_string())?
    };

    let dir_path = std::path::PathBuf::from(&dir);
    let dir_for_task = dir.clone();
    let task_app = app.clone();
    let (videos, stale_ids, warnings) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<Video>, Vec<String>, Vec<String>), String> {
            // 目录打不开时直接报错，绝不把"没扫到"当成"已删除"去清库
            let files = match scanner::scan_directory_recursive(&dir_path) {
                Ok(f) => f,
                Err(e) => return Err(format!("扫描中断，未修改数据库：{}", e)),
            };

            let found: HashSet<String> = files
                .iter()
                .map(|f| f.to_string_lossy().to_lowercase())
                .collect();
            let existing: Vec<_> = existing_videos.iter()
                .map(|v| (v.id.clone(), v.path.clone())).collect();
            let by_path: std::collections::HashMap<_, _> = existing_videos.iter()
                .map(|v| (v.path.to_lowercase(), v)).collect();

            // 本次扫描已找不到、但库里还挂在该目录下的文件 → 视为外部已删除
            let prefix = format!("{}\\", dir_for_task.trim_end_matches('\\').to_lowercase());
            let stale_ids = compute_stale_ids(&existing, &prefix, &found);

            let new_files: Vec<_> = files
                .into_iter()
                .filter(|f| {
                    match by_path.get(&f.to_string_lossy().to_lowercase()) {
                        None => true,
                        Some(old) => old.duration.is_none() || old.width.is_none() || old.height.is_none()
                            || std::fs::metadata(f).map(|m| m.len() as i64 != old.file_size).unwrap_or(true),
                    }
                })
                .collect();

            let _ = task_app.emit(
                "scan-progress",
                ScanProgress { processed: 0, total: new_files.len(), done: false, warnings: vec![] },
            );

            let (mut videos, warnings) = build_videos_parallel(&task_app, &new_files);
            for video in &mut videos {
                if let Some(old) = by_path.get(&video.path.to_lowercase()) {
                    let incomplete = video.duration.is_none() || video.width.is_none() || video.height.is_none();
                    video.id = old.id.clone();
                    video.path = old.path.clone();
                    video.created_at = old.created_at.clone();
                    video.duration = video.duration.or(old.duration);
                    video.width = video.width.or(old.width);
                    video.height = video.height.or(old.height);
                    // A failed refresh must remain eligible for retry on the next scan.
                    if incomplete {
                        video.file_size = old.file_size;
                    }
                }
            }
            Ok((videos, stale_ids, warnings))
        })
        .await
        .map_err(|e| e.to_string())??;

    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        // 清理失效条目 + 插入新条目必须在同一事务里：要么全部生效，要么全部回滚
        // （Transaction 会解引用为 Connection，直接复用现有函数）
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        if !stale_ids.is_empty() {
            db::delete_videos_by_ids(&tx, &stale_ids).map_err(|e| e.to_string())?;
        }
        for video in &videos {
            db::insert_video(&tx, video).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }

    let _ = app.emit(
        "scan-progress",
        ScanProgress { processed: videos.len(), total: videos.len(), done: true, warnings: warnings.clone() },
    );

    Ok(videos)
}

/// 返回 should-delete 的 video id：路径在 prefix 目录下（大小写不敏感），且不在 found 集合里。
/// prefix 已含结尾分隔符，避免 "D:\v" 误匹配 "D:\vids2"。
fn compute_stale_ids(
    existing: &[(String, String)],
    prefix: &str,
    found_lower: &HashSet<String>,
) -> Vec<String> {
    existing
        .iter()
        .filter(|(_, path)| {
            let lp = path.to_lowercase();
            lp.starts_with(prefix) && !found_lower.contains(&lp)
        })
        .map(|(id, _)| id.clone())
        .collect()
}

fn build_videos_parallel(
    app: &tauri::AppHandle,
    files: &[std::path::PathBuf],
) -> (Vec<Video>, Vec<String>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let total = files.len();
    let next = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let probe_failures = AtomicUsize::new(0);
    let thread_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .min(total);

    let videos = std::thread::scope(|s| {
        let handles: Vec<_> = (0..thread_count)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        let video = scanner::build_video(&files[i]);
                        if video.duration.is_none() {
                            probe_failures.fetch_add(1, Ordering::Relaxed);
                        }
                        out.push(video);
                        let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = app.emit(
                            "scan-progress",
                            ScanProgress { processed: done, total, done: false, warnings: vec![] },
                        );
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });

    let mut warnings = Vec::new();
    let failures = probe_failures.load(Ordering::Relaxed);
    if failures > 0 {
        warnings.push(format!(
            "{} 个视频未能获取时长（请确认已安装 ffmpeg/ffprobe，或文件本身损坏）",
            failures
        ));
    }
    (videos, warnings)
}

#[cfg(test)]
mod tests {
    use super::compute_stale_ids;

    #[test]
    fn test_compute_stale_ids() {
        let existing = vec![
            ("a".to_string(), r"D:\vid\one.mp4".to_string()),
            ("b".to_string(), r"D:\vid\sub\two.mp4".to_string()),
            ("c".to_string(), r"D:\vids2\three.mp4".to_string()),
            ("d".to_string(), r"C:\other\four.mp4".to_string()),
        ];
        // 目录里现在只有 one.mp4（大小写不同也要识别为存在）和 vids2 下的 three.mp4
        let found: std::collections::HashSet<String> = [r"D:\vid\ONE.mp4", r"D:\vids2\three.mp4"]
            .iter()
            .map(|s| s.to_lowercase())
            .collect();

        let stale = compute_stale_ids(&existing, r"d:\vid\", &found);
        assert_eq!(stale, vec!["b".to_string()]);
    }

    #[test]
    fn test_compute_stale_ids_prefix_boundary() {
        // "D:\v" 不能误匹配 "D:\vids2\x.mp4"
        let existing = vec![("c".to_string(), r"D:\vids2\three.mp4".to_string())];
        let found: std::collections::HashSet<String> = std::collections::HashSet::new();
        let stale = compute_stale_ids(&existing, r"d:\v\", &found);
        assert!(stale.is_empty());
    }
}
