use std::sync::Mutex;

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::scanner;

use super::videos::unique_path;
use super::{clear_cancel, is_cancelled, video_path_or, AppState, MapErrStr};

/// 封面扩展名：libwebp 可用即 webp（体积 -40%），否则回退 jpg。老 jpg 缓存照读不误。
pub(crate) fn thumb_ext() -> &'static str {
    if scanner::webp_available() {
        "webp"
    } else {
        "jpg"
    }
}

pub(crate) fn thumb_file(dir: &std::path::Path, id: &str) -> std::path::PathBuf {
    // 新缓存按当前编码器扩展名；老 jpg 缓存由 collect_orphan 的双后缀探测兼容
    dir.join(format!("{}.{}", id, thumb_ext()))
}

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
    let dir = super::cache_base(app).join("thumbnails");
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
/// 只碰文件系统不碰数据库——调用方必须先把数据库锁放掉再做这步逐条 stat。
/// 新老扩展名都认（webp 迁移前的 jpg 孤儿同样登记回来）。
fn collect_orphan_registers(
    dir: &std::path::Path,
    entries: &[(String, Option<String>)],
) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (id, path) in entries {
        if cached_thumbnail_usable(path) {
            continue;
        }
        let out = thumb_file(dir, id);
        let legacy = dir.join(format!("{}.jpg", id));
        let hit = if std::fs::metadata(&out)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            Some(out)
        } else if std::fs::metadata(&legacy)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            Some(legacy)
        } else {
            None
        };
        if let Some(p) = hit {
            found.push((id.clone(), p.to_string_lossy().to_string()));
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
    let Ok(tx) = conn.unchecked_transaction() else {
        return 0;
    };
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

/// 封面缓存文件名由条目 id 决定，条目移除后一并清理，避免留下孤儿文件。
/// 新老扩展名都清（webp 迁移前的 jpg 不留尾巴）。
pub(crate) fn clear_thumbnail_cache(app: &tauri::AppHandle, id: &str) {
    clear_thumbnail_cache_in(&super::cache_base(app), id);
}

/// 同上，但缓存目录由调用方给。**持着库锁时只能用这个，不能用上面那个**：
/// `cache_base` 自己要读 settings 会去 lock 那把 std Mutex，而 std Mutex 不可重入：
/// 持着锁再调它就是同线程二次上锁，永久阻塞。批量删除（图片/视频）正是这个形状：
/// 文件已进回收站、库记录已删，然后卡死在清封面这一步，命令永不返回，
/// 前端表现为「图删掉了，按钮一直停在删除中」。
pub(crate) fn clear_thumbnail_cache_in(cache_root: &std::path::Path, id: &str) {
    let base = cache_root.join("thumbnails");
    let _ = std::fs::remove_file(base.join(format!("{}.webp", id)));
    let _ = std::fs::remove_file(base.join(format!("{}.jpg", id)));
}

/// 启动时清一轮孤儿缩略图：id 在图片/视频两张表里都查无的缓存文件。
/// 视频与图片共用缓存目录，id 是 UUID 不会撞；id 还在库里的文件绝不能动——
/// 那是"抽好帧没来得及写库"的孤儿，resume_thumbnails 要靠它们登记回来。
/// 返回删除的文件数，失败（目录不存在等）一律返回 0，不影响启动。
pub(crate) fn cleanup_orphan_thumbnail_files(app: &tauri::AppHandle) -> usize {
    let dir = super::cache_base(app);
    let Some(state) = app.try_state::<AppState>() else {
        return 0;
    };
    let alive = {
        let Ok(conn) = state.db.lock() else { return 0 };
        let Ok(image_ids) = db::get_all_image_ids(&conn) else {
            return 0;
        };
        let Ok(video_ids) = db::get_all_video_ids(&conn) else {
            return 0;
        };
        image_ids
            .into_iter()
            .chain(video_ids)
            .collect::<std::collections::HashSet<_>>()
    };
    remove_orphan_thumbnail_files(&dir.join("thumbnails"), &alive)
}

/// 纯磁盘操作部分：删除 alive 集合之外的 `{id}.jpg`/`.webp`，以及主已不在的
/// `{id}.part.jpg`/`.part.webp`（抽帧中途崩溃留下的临时文件）。其他扩展一律不碰。
fn remove_orphan_thumbnail_files(
    dir: &std::path::Path,
    alive: &std::collections::HashSet<String>,
) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_thumb = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            e.eq_ignore_ascii_case("jpg")
                || e.eq_ignore_ascii_case("jpeg")
                || e.eq_ignore_ascii_case("webp")
        });
        if !is_thumb {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
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
            ThumbnailProgress {
                processed: 0,
                total: 0,
                done: true,
                generated: 0,
                failed: 0,
            },
        );
        return Ok(0);
    }

    let state = app.state::<AppState>();
    if !state.thumb_running.lock().map_err_str()?.insert(event) {
        return Err("该库已有封面生成任务在运行。".into());
    }
    let _guard = BatchGuard {
        running: &state.thumb_running,
        event,
    };

    // 落库"批次进行中"：进程中途被杀时标志留着，下次启动自动续跑。
    // 目录先取好——它若失败，标志已写却没人来清，会变成每次启动都重试的幽灵批次
    let dir = thumbnails_dir(app)?;
    let flag_key = thumb_flag_key(event);
    {
        let conn = state.db.lock().map_err_str()?;
        db::set_setting(&conn, &flag_key, event).map_err_str()?;
    }

    let task_app = app.clone();
    // 本轮取消标记先清掉（上一轮取消的残留不能带到这一轮）
    clear_cancel(app, event);
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
                        // 取消检查：前端 cancel_task 后当前批次内尽快停，不再开新 ffmpeg
                        if is_cancelled(&task_app, event) {
                            break;
                        }
                        let index = next_index.fetch_add(1, Ordering::SeqCst);
                        if index >= jobs.len() {
                            break;
                        }
                        let job = &jobs[index];
                        // 老 jpg 缓存仍有效则沿用（webp 迁移不强制重抽 19 万张）；
                        // 新抽的走当前编码器扩展名；登记时写实际存在的那一份路径
                        let legacy = dir.join(format!("{}.jpg", job.id));
                        let legacy_ok = std::fs::metadata(&legacy)
                            .map(|m| m.len() > 0)
                            .unwrap_or(false);
                        let out = thumb_file(&dir, &job.id);
                        // 上次中途关闭留下的孤儿文件：直接登记，不重新抽帧
                        let existing_ok = std::fs::metadata(&out)
                            .map(|m| m.len() > 0)
                            .unwrap_or(false);
                        let ready: Option<std::path::PathBuf> = if existing_ok {
                            Some(out)
                        } else if legacy_ok {
                            Some(legacy)
                        } else {
                            // 抽帧写临时名，成功才改名——ffmpeg 中途被杀不会留下半张图。
                            // 临时名与目标同扩展名：ffmpeg 按扩展名选封装格式，
                            // `{id}.part` 类无扩展名会直接失败。临时名带 id，多 worker 互不踩
                            let part = dir.join(format!("{}.part.{}", job.id, thumb_ext()));
                            let result =
                                scanner::extract_thumbnail(&job.source, &part, job.duration)
                                    .and_then(|()| std::fs::rename(&part, &out).map_err_str());
                            let _ = std::fs::remove_file(&part);
                            result.is_ok().then_some(out)
                        };
                        if let Some(final_path) = ready {
                            if let Ok(conn) = state.db.lock() {
                                let _ = persist(&conn, &job.id, &final_path.to_string_lossy());
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

        (
            generated.load(Ordering::SeqCst),
            failed.load(Ordering::SeqCst),
        )
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
        ThumbnailProgress {
            processed: total,
            total,
            done: true,
            generated,
            failed,
        },
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
            .map(|(id, source, duration, thumbnail_path)| ThumbEntry {
                id,
                source,
                duration,
                thumbnail_path,
            })
            .collect()
    };
    // 待办 id 一次建表查 O(1)：体检后要重建的是十万级全集，any() 的 O(n²) 会把命令卡死
    let wanted: std::collections::HashSet<&String> = video_ids.iter().collect();
    let jobs: Vec<ThumbJob> = candidates
        .into_iter()
        .filter(|e| wanted.contains(&e.id))
        // 缓存文件仍在的跳过；文件被删掉的会重新生成
        .filter(|e| !cached_thumbnail_usable(&e.thumbnail_path))
        .map(|e| ThumbJob {
            id: e.id,
            source: e.source,
            duration: e.duration,
        })
        .collect();

    run_thumbnail_jobs(&app, jobs, "thumbnail-progress", db::set_thumbnail).await
}

/// 缓存文件仍在磁盘上才算可用
pub(crate) fn cached_thumbnail_usable(path: &Option<String>) -> bool {
    matches!(path, Some(p) if std::path::Path::new(p).exists())
}

/// 死路径判定：库里记着的封面文件已经不在这盘上了。按封面所在目录分组，
/// 一个目录只读一次目录清单（12 万条逐条 stat 是几十秒，一次 read_dir 几秒）。
/// 目录本身读不到（可移动盘没插、缓存换了盘）整组跳过——那是"暂时看不见"，
/// 不是"没了"，跟着清就是把好封面全删了重抽。真清错了也还有救：缓存文件
/// 重新可见时 `resume_thumbnails` 的孤儿登记会按 id 认回来。
/// Windows 路径不分大小写，两边都转小写比文件名。
fn stale_thumbnail_ids(rows: &[(String, String)]) -> Vec<String> {
    let mut by_dir: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    for (id, path) in rows {
        let Some(file) = std::path::Path::new(path).file_name() else {
            continue;
        };
        let dir = std::path::Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        by_dir
            .entry(dir)
            .or_default()
            .push((id.clone(), file.to_string_lossy().to_lowercase()));
    }
    let mut stale = Vec::new();
    for (dir, group) in by_dir {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let present: std::collections::HashSet<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(|n| n.to_lowercase()))
            .collect();
        stale.extend(
            group
                .into_iter()
                .filter(|(_, name)| !present.contains(name))
                .map(|(id, _)| id),
        );
    }
    stale
}

/// 封面路径体检的回报：两张表各清掉多少条死路径
#[derive(Clone, Copy, serde::Serialize)]
pub struct ThumbnailRepairReport {
    pub images: usize,
    pub videos: usize,
}

/// 封面路径体检：把库里指向"已不在盘上的缓存文件"的死路径清成 NULL。
/// 缓存目录整个没了（换盘、手动清、误删）之后，库里 `thumbnail_path` 仍全有值，
/// 于是「缺封面」永远算出 0：工具栏「生成封面」按钮一直禁着、待办清单为空、
/// 卡片按死路径加载出 404 —— 整条重建通路被堵死。体检把库拉回与磁盘一致，
/// 计数与待办清单立刻恢复真实值，重建入口也就回来了。
/// 由前端启动时调一次（不放 setup：回报要递给界面，才能提示用户去重建）。
#[tauri::command]
pub async fn repair_thumbnail_paths(
    app: tauri::AppHandle,
) -> Result<ThumbnailRepairReport, String> {
    let report = tauri::async_runtime::spawn_blocking(move || -> Result<ThumbnailRepairReport, String> {
        use tauri::Manager;
        let state = app.state::<AppState>();
        // 锁内只拉 (id, 封面路径) 两列，逐组 read_dir 的文件系统活在锁外做
        let (image_rows, video_rows) = {
            let conn = state.db.lock().map_err_str()?;
            (
                db::list_image_thumbnail_paths(&conn).map_err_str()?,
                db::list_video_thumbnail_paths(&conn).map_err_str()?,
            )
        };
        let stale_images = stale_thumbnail_ids(&image_rows);
        let stale_videos = stale_thumbnail_ids(&video_rows);
        if stale_images.is_empty() && stale_videos.is_empty() {
            return Ok(ThumbnailRepairReport {
                images: 0,
                videos: 0,
            });
        }
        let conn = state.db.lock().map_err_str()?;
        let tx = conn.unchecked_transaction().map_err_str()?;
        let images = db::clear_thumbnail_paths(&tx, "images", &stale_images).map_err_str()?;
        let videos = db::clear_thumbnail_paths(&tx, "videos", &stale_videos).map_err_str()?;
        tx.commit().map_err_str()?;
        println!(
            "repaired stale thumbnail paths: {} images, {} videos",
            images, videos
        );
        Ok(ThumbnailRepairReport { images, videos })
    })
    .await
    .map_err_str()??;
    Ok(report)
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
                    .map(|(id, source, duration, thumbnail_path)| ThumbEntry {
                        id,
                        source,
                        duration,
                        thumbnail_path,
                    })
                    .collect()),
                _ => Ok(db::get_image_thumbnail_entries(&conn)
                    .map_err_str()?
                    .into_iter()
                    .map(|(id, source, thumbnail_path)| ThumbEntry {
                        id,
                        source,
                        duration: None,
                        thumbnail_path,
                    })
                    .collect()),
            }
        })
        .await
        .map_err_str()??;

    let orphan_entries: Vec<(String, Option<String>)> = entries
        .iter()
        .map(|e| (e.id.clone(), e.thumbnail_path.clone()))
        .collect();
    let registers = tauri::async_runtime::spawn_blocking(move || {
        collect_orphan_registers(&dir, &orphan_entries)
    })
    .await
    .map_err_str()?;
    let recovered = {
        let conn = state.db.lock().map_err_str()?;
        register_orphans(&conn, &registers, persist)
    };

    let flag_key = thumb_flag_key(event);
    let interrupted = {
        let conn = state.db.lock().map_err_str()?;
        db::get_setting(&conn, &flag_key).map_err_str()?.as_deref() == Some(event)
    };
    if !interrupted {
        return Ok(recovered > 0);
    }

    // 续跑批次：缓存文件不在盘上的才重跑。逐条 stat 拿的是上面拉出的行集，锁已放开
    let jobs: Vec<ThumbJob> = entries
        .into_iter()
        .filter(|e| !cached_thumbnail_usable(&e.thumbnail_path))
        .map(|e| ThumbJob {
            id: e.id,
            source: e.source,
            duration: e.duration,
        })
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
    use crate::db::{
        get_all_videos, insert_image, insert_video, sample_image, sample_video, setup_test_db,
    };

    /// 孤儿登记只认磁盘上真实存在的非空缓存文件；库里有可用封面的不碰
    #[test]
    fn test_recover_orphan_thumbnails_only_registers_real_files() {
        let conn = setup_test_db();
        let dir =
            std::env::temp_dir().join(format!("viewman-thumb-recover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for id in ["v1", "v2", "v3", "v4"] {
            insert_video(&conn, &sample_video(id, &format!(r"C:\media\{id}.mp4"))).unwrap();
        }
        std::fs::write(dir.join("v1.jpg"), [0u8; 10]).unwrap(); // 有文件的孤儿 → 登记
        std::fs::write(dir.join("v2.jpg"), b"").unwrap(); // 空文件不算抽帧成功 → 跳过
        let usable = dir.join("v3.jpg");
        std::fs::write(&usable, [0u8; 10]).unwrap(); // 库里已有可用封面 → 不碰
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
            vec![(
                "v1".to_string(),
                dir.join("v1.jpg").to_string_lossy().into_owned()
            )]
        );

        let recovered = register_orphans(&conn, &registers, db::set_thumbnail);
        assert_eq!(recovered, 1);

        let rows = get_all_videos(&conn).unwrap();
        let expected_v1 = dir.join("v1.jpg");
        assert_eq!(
            rows.iter()
                .find(|v| v.id == "v1")
                .unwrap()
                .thumbnail_path
                .as_deref(),
            Some(expected_v1.to_string_lossy().as_ref())
        );
        assert!(rows
            .iter()
            .find(|v| v.id == "v3")
            .unwrap()
            .thumbnail_path
            .is_some());

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
        let registers = vec![(
            "v1".to_string(),
            dir.join("v1.jpg").to_string_lossy().into_owned(),
        )];
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

        let alive: std::collections::HashSet<_> =
            ["a1", "a2"].into_iter().map(String::from).collect();
        std::fs::write(dir.join("a1.jpg"), [0u8; 4]).unwrap(); // 有主 → 保留
        std::fs::write(dir.join("a2.part.jpg"), [0u8; 4]).unwrap(); // 有主的 .part 残留 → 保留
        std::fs::write(dir.join("dead.jpg"), [0u8; 4]).unwrap(); // 无主 → 删
        std::fs::write(dir.join("dead.part.jpg"), [0u8; 4]).unwrap(); // 无主的 .part → 删
        std::fs::write(dir.join("dead.png"), [0u8; 4]).unwrap(); // 非 .jpg → 不碰

        let removed = remove_orphan_thumbnail_files(&dir, &alive);
        assert_eq!(removed, 2);
        assert!(dir.join("a1.jpg").exists());
        assert!(dir.join("a2.part.jpg").exists());
        assert!(dir.join("dead.png").exists());
        assert!(!dir.join("dead.jpg").exists());
        assert!(!dir.join("dead.part.jpg").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 死路径体检：目录读得到就按目录清单判——文件没了的才清，还在的一律不动
    /// （Windows 路径不分大小写，库里记 `CASED.JPG`、盘上是 `cased.jpg` 也算同一份）
    #[test]
    fn test_stale_thumbnail_ids_clears_only_missing_files() {
        let base =
            std::env::temp_dir().join(format!("viewman-thumb-repair-{}", std::process::id()));
        let dir = base.join("thumbnails");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alive.jpg"), [0u8; 8]).unwrap();
        std::fs::write(dir.join("cased.jpg"), [0u8; 8]).unwrap();

        let rows = vec![
            (
                "alive".to_string(),
                dir.join("alive.jpg").to_string_lossy().to_string(),
            ),
            (
                "gone".to_string(),
                dir.join("gone.jpg").to_string_lossy().to_string(),
            ),
            (
                "cased".to_string(),
                dir.join("CASED.JPG").to_string_lossy().to_string(),
            ),
        ];
        let mut stale = stale_thumbnail_ids(&rows);
        stale.sort();
        assert_eq!(stale, vec!["gone".to_string()]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 缓存目录整个读不到（可移动盘没插）时一条都不清：看不见 ≠ 没了
    #[test]
    fn test_stale_thumbnail_ids_skips_unreadable_dir() {
        let rows = vec![(
            "v1".to_string(),
            r"D:\viewman-没有这个目录-修复测试\thumbnails\v1.jpg".to_string(),
        )];
        assert!(stale_thumbnail_ids(&rows).is_empty());
    }

    /// 清掉死路径后「缺封面」立刻数得到、待办清单也捞得到——重建入口恢复
    #[test]
    fn test_clear_thumbnail_paths_reopens_regeneration_path() {
        let conn = setup_test_db();
        insert_image(&conn, &sample_image("i1", r"C:\pics\a.png")).unwrap();
        insert_image(&conn, &sample_image("i2", r"C:\pics\b.png")).unwrap();
        db::set_image_thumbnail(&conn, "i1", r"D:\viewman_cache\thumbnails\i1.jpg").unwrap();
        db::set_image_thumbnail(&conn, "i2", r"D:\viewman_cache\thumbnails\i2.jpg").unwrap();
        assert!(db::get_missing_image_thumbnail_ids(&conn)
            .unwrap()
            .is_empty());

        let cleared =
            db::clear_thumbnail_paths(&conn, "images", &["i1".to_string(), "i2".to_string()])
                .unwrap();
        assert_eq!(cleared, 2);
        let mut missing = db::get_missing_image_thumbnail_ids(&conn).unwrap();
        missing.sort();
        assert_eq!(missing, vec!["i1".to_string(), "i2".to_string()]);
        // 清单只给有路径的行，清空后不再出现在体检输入里
        assert!(db::list_image_thumbnail_paths(&conn).unwrap().is_empty());
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
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "frame".into());
        if !position.is_finite() || position < 0.0 {
            return Err("截图位置无效".into());
        }
        // 文件名带毫秒：只到秒的话同一位置重复截图会静默覆盖上一张。
        // 用总毫秒拆分时/分/秒，避免 (position%60).round()==60 的非法秒值（如 61.9s→01m62s）。
        let stamp = if position > 0.0 {
            let total_ms = (position * 1000.0).round().max(0.0) as u64;
            format!(
                "_{:02}m{:02}s{:03}",
                total_ms / 60000,
                (total_ms % 60000) / 1000,
                total_ms % 1000
            )
        } else {
            "_start".to_string()
        };
        // 同一位置重复截图不再 -y 静默覆盖：自动加 " (2)" 后缀
        let out = unique_path(&parent.join(format!("{}_{}.png", stem, stamp)))?;
        // 先写 .viewman-tmp- 临时名再 rename：中断/失败不留半个 png 在用户媒体
        // 目录里（半个 png 会被下一轮图片扫描收进库）
        let tmp = out.with_file_name(format!(
            ".viewman-tmp-{}",
            out.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("frame.png")
        ));

        // -ss 0 不传：ffmpeg 62 对 image2/mjpeg 的 0 偏移 seek 会跳过唯一一帧（同 extract_thumbnail）
        // 不传 -y：输出路径已由 unique_path 保证不重名，-y 会掩盖并发竞态的覆盖
        let mut cmd = crate::scanner::hidden_command("ffmpeg");
        if position > 0.0 {
            cmd.args(["-ss", &position.to_string()]);
        }
        cmd.args(["-i", &path, "-frames:v", "1", &tmp.to_string_lossy()]);
        let output = crate::scanner::output_timeout(cmd, 60, "ffmpeg 截图")?;
        let cleanup = |e: String| -> String {
            let _ = std::fs::remove_file(&tmp);
            e
        };
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(cleanup(format!(
                "截图失败: {}",
                detail.chars().take(200).collect::<String>()
            )));
        }
        match std::fs::metadata(&tmp) {
            Ok(m) if m.len() > 0 => {}
            _ => return Err(cleanup("ffmpeg 未产生有效截图".into())),
        }
        std::fs::rename(&tmp, &out).map_err(|e| cleanup(format!("落盘失败: {}", e)))?;
        Ok(out.to_string_lossy().to_string())
    })
    .await
    .map_err_str()?
}
