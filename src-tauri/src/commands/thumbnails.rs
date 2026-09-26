use std::sync::Mutex;

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::scanner;

use super::{AppState, MapErrStr};

#[derive(Clone, serde::Serialize)]
pub struct ThumbnailProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    pub generated: usize,
    pub failed: usize,
}

/// 一张待生成封面的源文件：图片没有时长，duration 传 None 即走首帧=整图缩放
pub(crate) struct ThumbJob {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) duration: Option<f64>,
}

/// 缩略图缓存目录：`<app_data>/thumbnails`
pub(crate) fn thumbnails_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err_str()?.join("thumbnails");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建缩略图目录失败: {}", e))?;
    Ok(dir)
}

/// settings 表里"封面批次进行中"的键前缀，完整键按事件名区分（视频/图片批次
/// 可以并发跑，共用一个键会互相覆盖：先跑完的清标志会把还在跑的那个的标志抹掉）。
/// 值是进度事件名，空串=没有批次在跑。正常结束会清掉；进程被杀就留着，
/// 下次启动 `resume_thumbnails` 据此续跑。
const THUMB_BATCH_FLAG: &str = "thumbnail_batch_running";

fn thumb_flag_key(event: &str) -> String {
    format!("{}.{}", THUMB_BATCH_FLAG, event)
}

type PersistFn = fn(&rusqlite::Connection, &str, &str) -> rusqlite::Result<()>;

/// 批次进行中的进程内占位：落库标志管跨重启，这个管单次进程内的并发
struct BatchGuard<'a> {
    running: &'a Mutex<std::collections::HashSet<&'static str>>,
    event: &'static str,
}

impl Drop for BatchGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.running.lock() {
            set.remove(&self.event);
        }
    }
}

/// 上次抽好帧却没来得及写库的孤儿缓存：文件还在就直接登记，不再抽帧。
/// 纯数据库操作，中断前生成的那部分进度即刻恢复可见。
fn recover_orphan_thumbnails(
    conn: &rusqlite::Connection,
    dir: &std::path::Path,
    entries: &[(String, Option<String>)],
    persist: PersistFn,
) -> usize {
    let mut recovered = 0;
    for (id, path) in entries {
        if cached_thumbnail_usable(path) {
            continue;
        }
        let out = dir.join(format!("{}.jpg", id));
        let existing = std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false);
        if existing && persist(conn, id, &out.to_string_lossy()).is_ok() {
            recovered += 1;
        }
    }
    recovered
}

/// 封面缓存文件名由条目 id 决定，条目移除后一并清理，避免留下孤儿文件
pub(crate) fn clear_thumbnail_cache(app: &tauri::AppHandle, id: &str) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join("thumbnails").join(format!("{}.jpg", id)));
    }
}

/// 封面批量生成：有界 worker 池并行抽帧，每张完成立即写库。
/// 抽帧过程不持有数据库锁，中途关闭应用已完成的部分不丢。
/// 视频与图片共用这条通路，差异只在事件名与 `persist` 写的是哪张表。
pub(crate) async fn run_thumbnail_jobs(
    app: &tauri::AppHandle,
    jobs: Vec<ThumbJob>,
    event: &'static str,
    persist: fn(&rusqlite::Connection, &str, &str) -> rusqlite::Result<()>,
) -> Result<usize, String> {
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法生成封面。请安装 ffmpeg 并加入 PATH。".into());
    }

    let total = jobs.len();
    if total == 0 {
        let _ = app.emit(
            event,
            ThumbnailProgress { processed: 0, total: 0, done: true, generated: 0, failed: 0 },
        );
        return Ok(0);
    }

    let state = app.state::<AppState>();
    if !state.thumb_running.lock().map_err_str()?.insert(event) {
        return Err("该库已有封面生成任务在运行。".into());
    }
    let _guard = BatchGuard { running: &state.thumb_running, event };

    // 落库"批次进行中"：进程中途被杀时标志留着，下次启动自动续跑。
    // 目录先取好——它若失败，标志已写却没人来清，会变成每次启动都重试的幽灵批次
    let dir = thumbnails_dir(app)?;
    let flag_key = thumb_flag_key(event);
    {
        let conn = state.db.lock().map_err_str()?;
        db::set_setting(&conn, &flag_key, event).map_err_str()?;
    }

    let task_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || -> (usize, usize) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tauri::Manager;

        // worker 数封顶 4：视频抽帧靠 seek，并发再高只会让机械盘来回找磁头，
        // SSD 也吃不到更多收益（ffmpeg 本身单核就够）
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .min(4);
        let state = task_app.state::<AppState>();
        let next_index = AtomicUsize::new(0);
        let processed = AtomicUsize::new(0);
        let emitted = AtomicUsize::new(0);
        let generated = AtomicUsize::new(0);
        let failed = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let index = next_index.fetch_add(1, Ordering::SeqCst);
                        if index >= jobs.len() {
                            break;
                        }
                        let job = &jobs[index];
                        let out = dir.join(format!("{}.jpg", job.id));
                        // 上次中途关闭留下的孤儿文件：直接登记，不重新抽帧
                        let existing_ok =
                            std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false);
                        let ok = existing_ok || {
                            // 抽帧写临时名，成功才改名——ffmpeg 中途被杀不会留下半张 jpg。
                            // 临时名必须以 .jpg 结尾：ffmpeg 按扩展名选封装格式，
                            // `.jpg.part` 会直接失败。临时名带 id，多 worker 互不踩
                            let part = dir.join(format!("{}.part.jpg", job.id));
                            let result = scanner::extract_thumbnail(&job.source, &part, job.duration)
                                .and_then(|()| std::fs::rename(&part, &out).map_err_str());
                            let _ = std::fs::remove_file(&part);
                            result.is_ok()
                        };
                        if ok {
                            if let Ok(conn) = state.db.lock() {
                                let _ = persist(&conn, &job.id, &out.to_string_lossy());
                            }
                            generated.fetch_add(1, Ordering::SeqCst);
                        } else {
                            failed.fetch_add(1, Ordering::SeqCst);
                        }
                        let at = processed.fetch_add(1, Ordering::SeqCst) + 1;
                        // 多 worker 下完成顺序和序号无关，后开工的可能先算完：
                        // 序号没超过已发过的最大值就不发，进度条不能回跳
                        if at > emitted.fetch_max(at, Ordering::SeqCst) {
                            let _ = task_app.emit(
                                event,
                                ThumbnailProgress {
                                    processed: at,
                                    total,
                                    done: false,
                                    generated: generated.load(Ordering::SeqCst),
                                    failed: failed.load(Ordering::SeqCst),
                                },
                            );
                        }
                    }
                });
            }
        });

        (generated.load(Ordering::SeqCst), failed.load(Ordering::SeqCst))
    })
    .await;

    // 无论成败都清标志：spawn 失败也不能给下次启动留一个幽灵批次
    {
        let conn = state.db.lock().map_err_str()?;
        let _ = db::set_setting(&conn, &thumb_flag_key(event), "");
    }

    let (generated, failed) = joined.map_err_str()?;

    let _ = app.emit(
        event,
        ThumbnailProgress { processed: total, total, done: true, generated, failed },
    );

    Ok(generated)
}

/// 为指定视频生成封面（ffmpeg 抽帧），已有有效缓存文件的会跳过
#[tauri::command]
pub async fn generate_thumbnails(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    video_ids: Vec<String>,
) -> Result<usize, String> {
    let jobs: Vec<ThumbJob> = {
        let conn = state.db.lock().map_err_str()?;
        let all = db::get_all_videos(&conn).map_err_str()?;
        all.into_iter()
            .filter(|v| video_ids.iter().any(|id| id == &v.id))
            // 缓存文件仍在的跳过；文件被删掉的会重新生成
            .filter(|v| !cached_thumbnail_usable(&v.thumbnail_path))
            .map(|v| ThumbJob { id: v.id, source: v.path, duration: v.duration })
            .collect()
    };

    run_thumbnail_jobs(&app, jobs, "thumbnail-progress", db::set_thumbnail).await
}

/// 缓存文件仍在磁盘上才算可用
pub(crate) fn cached_thumbnail_usable(path: &Option<String>) -> bool {
    matches!(path, Some(p) if std::path::Path::new(p).exists())
}

/// 启动续跑：先登记上次中断留下的孤儿缓存（不抽帧，纯写库），再接着跑没跑完的批次。
/// 返回是否做了恢复，前端据此刷新列表并提示。
#[tauri::command]
pub async fn resume_thumbnails(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    kind: String,
) -> Result<bool, String> {
    let (event, persist): (&'static str, PersistFn) = match kind.as_str() {
        "video" => ("thumbnail-progress", db::set_thumbnail),
        "image" => ("image-thumbnail-progress", db::set_image_thumbnail),
        other => return Err(format!("未知的媒体类型: {}", other)),
    };

    let dir = thumbnails_dir(&app)?;

    // 孤儿登记不管标志：上次批次哪怕是别的库的，这边抽好帧没写库的也一样收回来
    let recovered = {
        let conn = state.db.lock().map_err_str()?;
        match kind.as_str() {
            "video" => {
                let entries: Vec<(String, Option<String>)> = db::get_all_videos(&conn)
                    .map_err_str()?
                    .into_iter()
                    .map(|v| (v.id, v.thumbnail_path))
                    .collect();
                recover_orphan_thumbnails(&conn, &dir, &entries, persist)
            }
            _ => {
                let entries: Vec<(String, Option<String>)> = db::get_all_images(&conn)
                    .map_err_str()?
                    .into_iter()
                    .map(|i| (i.id, i.thumbnail_path))
                    .collect();
                recover_orphan_thumbnails(&conn, &dir, &entries, persist)
            }
        }
    };

    let flag_key = thumb_flag_key(event);
    let interrupted = {
        let conn = state.db.lock().map_err_str()?;
        db::get_setting(&conn, &flag_key)
            .map_err_str()?
            .as_deref()
            == Some(event)
    };
    if !interrupted {
        return Ok(recovered > 0);
    }

    let jobs: Vec<ThumbJob> = {
        let conn = state.db.lock().map_err_str()?;
        match kind.as_str() {
            "video" => db::get_all_videos(&conn)
                .map_err_str()?
                .into_iter()
                .filter(|v| !cached_thumbnail_usable(&v.thumbnail_path))
                .map(|v| ThumbJob { id: v.id, source: v.path, duration: v.duration })
                .collect(),
            _ => db::get_all_images(&conn)
                .map_err_str()?
                .into_iter()
                .filter(|i| !cached_thumbnail_usable(&i.thumbnail_path))
                .map(|i| ThumbJob { id: i.id, source: i.path, duration: None })
                .collect(),
        }
    };
    if jobs.is_empty() {
        // 标志还挂着但活儿已经没了：清掉，免得每次启动都白查一遍
        let conn = state.db.lock().map_err_str()?;
        let _ = db::set_setting(&conn, &flag_key, "");
        return Ok(recovered > 0);
    }

    // 不在这里清标志：run_thumbnail_jobs 开始时会重新写上、结束时会清掉；
    // 它若没跑起来（ffmpeg 缺失/同库已在跑），标志留着，下次启动再试
    run_thumbnail_jobs(&app, jobs, event, persist).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{get_all_videos, insert_video, sample_video, setup_test_db};

    /// 孤儿登记只认磁盘上真实存在的非空缓存文件；库里有可用封面的不碰
    #[test]
    fn test_recover_orphan_thumbnails_only_registers_real_files() {
        let conn = setup_test_db();
        let dir = std::env::temp_dir().join(format!("viewman-thumb-recover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for id in ["v1", "v2", "v3", "v4"] {
            insert_video(&conn, &sample_video(id, &format!(r"C:\media\{id}.mp4"))).unwrap();
        }
        std::fs::write(dir.join("v1.jpg"), [0u8; 10]).unwrap();  // 有文件的孤儿 → 登记
        std::fs::write(dir.join("v2.jpg"), b"").unwrap();        // 空文件不算抽帧成功 → 跳过
        let usable = dir.join("v3.jpg");
        std::fs::write(&usable, [0u8; 10]).unwrap();             // 库里已有可用封面 → 不碰
        // v4 磁盘上没有文件 → 跳过
        db::set_thumbnail(&conn, "v3", &usable.to_string_lossy()).unwrap();

        let entries = vec![
            ("v1".into(), None),
            ("v2".into(), None),
            ("v3".into(), Some(usable.to_string_lossy().into_owned())),
            ("v4".into(), None),
        ];
        let recovered = recover_orphan_thumbnails(&conn, &dir, &entries, db::set_thumbnail);
        assert_eq!(recovered, 1);

        let rows = get_all_videos(&conn).unwrap();
        let expected_v1 = dir.join("v1.jpg");
        assert_eq!(
            rows.iter().find(|v| v.id == "v1").unwrap().thumbnail_path.as_deref(),
            Some(expected_v1.to_string_lossy().as_ref())
        );
        assert!(rows.iter().find(|v| v.id == "v3").unwrap().thumbnail_path.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 播放器截图：截当前播放帧，存到视频同目录（<视频名>_<时间戳>.png），返回输出路径。
/// 用 ffmpeg seek 到指定秒抽 1 帧，原始分辨率不缩放
#[tauri::command]
pub async fn capture_frame(
    state: State<'_, AppState>,
    video_id: String,
    position: f64,
) -> Result<String, String> {
    let path = {
        let conn = state.db.lock().map_err_str()?;
        db::get_video_path(&conn, &video_id)
            .map_err_str()?
            .ok_or_else(|| format!("Video not found: {}", video_id))?
    };

    tauri::async_runtime::spawn_blocking(move || {
        let src = std::path::Path::new(&path);
        let parent = src.parent().ok_or("无法确定视频所在目录")?;
        let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "frame".into());
        let stamp = if position > 0.0 { format!("_{:02}m{:02}s", (position / 60.0).floor() as u32, (position % 60.0).round() as u32) } else { "_start".to_string() };
        let out = parent.join(format!("{}_{}.png", stem, stamp));

        let output = crate::scanner::hidden_command("ffmpeg")
            .args([
                "-y",
                "-ss", &position.max(0.0).to_string(),
                "-i", &path,
                "-frames:v", "1",
                &out.to_string_lossy(),
            ])
            .output()
            .map_err(|e| format!("无法运行 ffmpeg: {}", e))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(format!("截图失败: {}", detail.chars().take(200).collect::<String>()));
        }
        Ok(out.to_string_lossy().to_string())
    })
    .await
    .map_err_str()?
}
