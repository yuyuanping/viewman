use std::sync::Mutex;

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::scanner;

use super::{video_path_or, AppState, MapErrStr};

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

/// 封面批次待办的最小行集：续跑与生成前要逐条 stat 缓存文件在不在，
/// 这一步是文件系统活，必须挪到数据库锁外做（19 万条就是几十秒，锁内做会把
/// 视图查询、启动重扫全堵死）——锁内只拉这几列，锁由调用方尽快放手
pub(crate) struct ThumbEntry {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) duration: Option<f64>,
    pub(crate) thumbnail_path: Option<String>,
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

/// 上次抽好帧却没来得及写库的孤儿缓存：文件还在就收集起来，不再抽帧。
/// 只碰文件系统不碰数据库——调用方必须先把数据库锁放掉再做这步逐条 stat
fn collect_orphan_registers(
    dir: &std::path::Path,
    entries: &[(String, Option<String>)],
) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (id, path) in entries {
        if cached_thumbnail_usable(path) {
            continue;
        }
        let out = dir.join(format!("{}.jpg", id));
        if std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false) {
            found.push((id.clone(), out.to_string_lossy().to_string()));
        }
    }
    found
}

/// 纯数据库部分：一个事务把收集到的孤儿缓存登记回库，中断前生成的那部分进度即刻恢复可见
fn register_orphans(
    conn: &rusqlite::Connection,
    updates: &[(String, String)],
    persist: PersistFn,
) -> usize {
    if updates.is_empty() {
        return 0;
    }
    let Ok(tx) = conn.unchecked_transaction() else { return 0 };
    let mut registered = 0;
    for (id, path) in updates {
        if persist(&tx, id, path).is_ok() {
            registered += 1;
        }
    }
    if tx.commit().is_err() {
        return 0;
    }
    registered
}

/// 封面缓存文件名由条目 id 决定，条目移除后一并清理，避免留下孤儿文件
pub(crate) fn clear_thumbnail_cache(app: &tauri::AppHandle, id: &str) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join("thumbnails").join(format!("{}.jpg", id)));
    }
}

/// 启动时清一轮孤儿缩略图：id 在图片/视频两张表里都查无的缓存文件。
/// 视频与图片共用缓存目录，id 是 UUID 不会撞；id 还在库里的文件绝不能动——
/// 那是"抽好帧没来得及写库"的孤儿，resume_thumbnails 要靠它们登记回来。
/// 返回删除的文件数，失败（目录不存在等）一律返回 0，不影响启动。
pub(crate) fn cleanup_orphan_thumbnail_files(app: &tauri::AppHandle) -> usize {
    let Ok(dir) = app.path().app_data_dir() else { return 0 };
    let Some(state) = app.try_state::<AppState>() else { return 0 };
    let alive = {
        let Ok(conn) = state.db.lock() else { return 0 };
        let Ok(image_ids) = db::get_all_image_ids(&conn) else { return 0 };
        let Ok(video_ids) = db::get_all_video_ids(&conn) else { return 0 };
        image_ids.into_iter().chain(video_ids).collect::<std::collections::HashSet<_>>()
    };
    remove_orphan_thumbnail_files(&dir.join("thumbnails"), &alive)
}

/// 纯磁盘操作部分：删除 alive 集合之外的 `{id}.jpg`，以及主已不在的 `{id}.part.jpg`
/// （抽帧中途崩溃留下的临时文件）。非 .jpg 的东西一律不碰。
fn remove_orphan_thumbnail_files(dir: &std::path::Path, alive: &std::collections::HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| !e.eq_ignore_ascii_case("jpg")).unwrap_or(true) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let id = stem.strip_suffix(".part").unwrap_or(stem);
        if !alive.contains(id) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
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
    // 最小行集在锁内一次拉完；"缓存文件还在吗"的逐条 stat 挪到锁外，
    // 免得几千上万次的 metadata 让其它命令排队等数据库锁
    let candidates: Vec<ThumbEntry> = {
        let conn = state.db.lock().map_err_str()?;
        db::get_video_thumbnail_entries(&conn)
            .map_err_str()?
            .into_iter()
            .map(|(id, source, duration, thumbnail_path)| ThumbEntry { id, source, duration, thumbnail_path })
            .collect()
    };
    let jobs: Vec<ThumbJob> = candidates
        .into_iter()
        .filter(|e| video_ids.iter().any(|id| id == &e.id))
        // 缓存文件仍在的跳过；文件被删掉的会重新生成
        .filter(|e| !cached_thumbnail_usable(&e.thumbnail_path))
        .map(|e| ThumbJob { id: e.id, source: e.source, duration: e.duration })
        .collect();

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

    // 孤儿登记不管标志：上次批次哪怕是别的库的，这边抽好帧没写库的也一样收回来。
    // 数据库锁里只做这一下最小行集查询；逐条 stat 的文件系统活在锁外做——
    // 19 万条 metadata 是几十秒的活，锁内做会把启动重扫和视图查询全堵在这把锁后面
    let task_app = app.clone();
    let task_kind = kind.clone();
    let entries: Vec<ThumbEntry> =
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<ThumbEntry>, String> {
            let state = task_app.state::<AppState>();
            let conn = state.db.lock().map_err_str()?;
            match task_kind.as_str() {
                "video" => Ok(db::get_video_thumbnail_entries(&conn)
                    .map_err_str()?
                    .into_iter()
                    .map(|(id, source, duration, thumbnail_path)| ThumbEntry { id, source, duration, thumbnail_path })
                    .collect()),
                _ => Ok(db::get_image_thumbnail_entries(&conn)
                    .map_err_str()?
                    .into_iter()
                    .map(|(id, source, thumbnail_path)| ThumbEntry { id, source, duration: None, thumbnail_path })
                    .collect()),
            }
        })
        .await
        .map_err_str()??;

    let orphan_entries: Vec<(String, Option<String>)> = entries
        .iter()
        .map(|e| (e.id.clone(), e.thumbnail_path.clone()))
        .collect();
    let registers =
        tauri::async_runtime::spawn_blocking(move || collect_orphan_registers(&dir, &orphan_entries))
            .await
            .map_err_str()?;
    let recovered = {
        let conn = state.db.lock().map_err_str()?;
        register_orphans(&conn, &registers, persist)
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

    // 续跑批次：缓存文件不在盘上的才重跑。逐条 stat 拿的是上面拉出的行集，锁已放开
    let jobs: Vec<ThumbJob> = entries
        .into_iter()
        .filter(|e| !cached_thumbnail_usable(&e.thumbnail_path))
        .map(|e| ThumbJob { id: e.id, source: e.source, duration: e.duration })
        .collect();
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
        let registers = collect_orphan_registers(&dir, &entries);
        assert_eq!(
            registers,
            vec![("v1".to_string(), dir.join("v1.jpg").to_string_lossy().into_owned())]
        );

        let recovered = register_orphans(&conn, &registers, db::set_thumbnail);
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

    /// 登记走一个事务：整批要么都进库，要么一条不留
    #[test]
    fn test_register_orphans_is_empty_safe() {
        let conn = setup_test_db();
        assert_eq!(register_orphans(&conn, &[], db::set_thumbnail), 0);

        let dir = std::env::temp_dir().join(format!("viewman-thumb-reg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        insert_video(&conn, &sample_video("v1", r"C:\media\v1.mp4")).unwrap();
        let registers = vec![("v1".to_string(), dir.join("v1.jpg").to_string_lossy().into_owned())];
        assert_eq!(register_orphans(&conn, &registers, db::set_thumbnail), 1);
        assert_eq!(
            get_all_videos(&conn).unwrap()[0].thumbnail_path.as_deref(),
            Some(dir.join("v1.jpg").to_string_lossy().as_ref())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 清理只删两张表都查无此 id 的缓存文件；有主的（含 .part 临时名）和非 .jpg 不碰
    #[test]
    fn test_remove_orphan_thumbnail_files_keeps_alive_ids() {
        let dir = std::env::temp_dir().join(format!("viewman-thumb-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let alive: std::collections::HashSet<_> = ["a1", "a2"].into_iter().map(String::from).collect();
        std::fs::write(dir.join("a1.jpg"), [0u8; 4]).unwrap();      // 有主 → 保留
        std::fs::write(dir.join("a2.part.jpg"), [0u8; 4]).unwrap(); // 有主的 .part 残留 → 保留
        std::fs::write(dir.join("dead.jpg"), [0u8; 4]).unwrap();    // 无主 → 删
        std::fs::write(dir.join("dead.part.jpg"), [0u8; 4]).unwrap(); // 无主的 .part → 删
        std::fs::write(dir.join("dead.png"), [0u8; 4]).unwrap();    // 非 .jpg → 不碰

        let removed = remove_orphan_thumbnail_files(&dir, &alive);
        assert_eq!(removed, 2);
        assert!(dir.join("a1.jpg").exists());
        assert!(dir.join("a2.part.jpg").exists());
        assert!(dir.join("dead.png").exists());
        assert!(!dir.join("dead.jpg").exists());
        assert!(!dir.join("dead.part.jpg").exists());

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
        video_path_or(&conn, &video_id)?
    };

    tauri::async_runtime::spawn_blocking(move || {
        let src = std::path::Path::new(&path);
        let parent = src.parent().ok_or("无法确定视频所在目录")?;
        let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "frame".into());
        // 文件名带毫秒：只到秒的话同一位置重复截图会静默覆盖上一张
        let stamp = if position > 0.0 {
            let ms = ((position * 1000.0).round() % 1000.0) as u32;
            format!(
                "_{:02}m{:02}s{:03}",
                (position / 60.0).floor() as u32,
                (position % 60.0).round() as u32,
                ms
            )
        } else {
            "_start".to_string()
        };
        let out = parent.join(format!("{}_{}.png", stem, stamp));

        // -ss 0 不传：ffmpeg 62 对 image2/mjpeg 的 0 偏移 seek 会跳过唯一一帧（同 extract_thumbnail）
        let mut cmd = crate::scanner::hidden_command("ffmpeg");
        cmd.arg("-y");
        if position > 0.0 {
            cmd.args(["-ss", &position.to_string()]);
        }
        let output = cmd
            .args([
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
