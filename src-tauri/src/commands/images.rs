use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::models::{Image, ScanOutcome};
use crate::scanner;

use super::scan::{
    capped_walk_warnings, dir_prefix_lower, plan_stale_ids, ScanProgress, ScanSummary,
};
use super::settings::{remember_root, IMAGE_SCAN_ROOTS_KEY};
use super::thumbnails::{cached_thumbnail_usable, clear_thumbnail_cache, run_thumbnail_jobs, ThumbJob};
use super::videos::move_file;
use super::{read_cache, remove_cache, undeleted_targets, write_cache, AppState};
use super::{DUPLICATE_CACHE, SIMILAR_CACHE, VIDEO_DUPLICATE_CACHE};

#[tauri::command]
pub fn get_images(state: State<AppState>) -> Result<Vec<Image>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_all_images(&conn).map_err(|e| e.to_string())
}

/// 递归扫描图片目录并增量更新图片库：新文件探测尺寸，扫描时已消失的文件从库里清掉。
/// 与视频扫描各走各的表，同一个目录可以被两边分别收录。
#[tauri::command]
pub async fn scan_image_directory(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<ScanOutcome<Image>, String> {
    let existing_images = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_all_images(&conn).map_err(|e| e.to_string())?
    };

    let dir_path = PathBuf::from(&dir);
    let dir_for_task = dir.clone();
    let task_app = app.clone();
    let (images, stale_ids, warnings, summary) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<Image>, Vec<String>, Vec<String>, ScanSummary), String> {
            // 根目录打不开时直接报错，绝不把"没扫到"当成"已删除"去清库
            let walk = match scanner::scan_image_directory_recursive(&dir_path) {
                Ok(w) => w,
                Err(e) => return Err(format!("扫描中断，未修改数据库：{}", e)),
            };
            let files = walk.files;
            let mut walk_warnings = capped_walk_warnings(walk.warnings);
            // 读得到目录就先记下根：本轮扫描即使被打断，下次启动也会接着扫
            if let Ok(conn) = task_app.state::<AppState>().db.lock() {
                let _ = remember_root(&conn, IMAGE_SCAN_ROOTS_KEY, &dir_for_task);
            }

            let found: HashSet<String> = files
                .iter()
                .map(|f| f.to_string_lossy().to_lowercase())
                .collect();
            let existing: Vec<_> = existing_images
                .iter()
                .map(|i| (i.id.clone(), i.path.clone()))
                .collect();
            let by_path: HashMap<_, _> = existing_images
                .iter()
                .map(|i| (i.path.to_lowercase(), i))
                .collect();

            // 本次扫描已找不到、但库里还挂在该目录下的文件 → 视为外部已删除
            let prefix = dir_prefix_lower(&dir_for_task);
            let stale_ids = plan_stale_ids(
                &existing,
                &prefix,
                &found,
                &walk.skipped,
                &mut walk_warnings,
            );

            let new_files: Vec<_> = files
                .into_iter()
                .filter(|f| {
                    image_needs_probe(
                        by_path.get(&f.to_string_lossy().to_lowercase()).copied(),
                        std::fs::metadata(f).ok().map(|m| m.len() as i64),
                    )
                })
                .collect();

            let _ = task_app.emit(
                "image-scan-progress",
                ScanProgress { processed: 0, total: new_files.len(), done: false, warnings: vec![], summary: None },
            );

            let (mut images, probe_warnings) = build_images_parallel(&task_app, &new_files);
            let mut warnings = walk_warnings;
            warnings.extend(probe_warnings);
            let mut refreshed = 0;
            for image in &mut images {
                if let Some(old) = by_path.get(&image.path.to_lowercase()) {
                    refreshed += 1;
                    let incomplete = image.width.is_none() || image.height.is_none();
                    image.id = old.id.clone();
                    image.path = old.path.clone();
                    image.created_at = old.created_at.clone();
                    image.width = image.width.or(old.width);
                    image.height = image.height.or(old.height);
                    // 探测失败的条目不能被当成"大小没变"，否则下次扫描不会再试
                    if incomplete {
                        image.file_size = old.file_size;
                    }
                }
            }
            let added = images.iter().filter(|i| !by_path.contains_key(&i.path.to_lowercase())).count();
            Ok((images, stale_ids, warnings, ScanSummary { added, removed: 0, refreshed }))
        })
        .await
        .map_err(|e| e.to_string())??;

    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        // 新条目已在探测时逐张落库，这里只清外部已删除的失效条目
        if !stale_ids.is_empty() {
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            db::delete_images_by_ids(&tx, &stale_ids).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
    }

    let _ = app.emit(
        "image-scan-progress",
        ScanProgress { processed: images.len(), total: images.len(), done: true, warnings: warnings.clone(), summary: Some(ScanSummary { added: summary.added, removed: stale_ids.len(), refreshed: summary.refreshed }) },
    );

    Ok(ScanOutcome { items: images, removed_ids: stale_ids })
}

/// 要不要重新探测：库里没有这条、探测过但没拿到尺寸、或文件大小变了。
/// 已探测过的直接跳过，这就是"扫描被打断后接着扫而不是从头扫"的依据。
fn image_needs_probe(old: Option<&Image>, size: Option<i64>) -> bool {
    match old {
        None => true,
        Some(entry) => {
            entry.width.is_none() || entry.height.is_none() || size != Some(entry.file_size)
        }
    }
}

fn build_images_parallel(
    app: &tauri::AppHandle,
    files: &[PathBuf],
) -> (Vec<Image>, Vec<String>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let total = files.len();
    let next = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let probe_failures = AtomicUsize::new(0);
    let write_failures = AtomicUsize::new(0);
    let thread_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .min(total);

    let images = std::thread::scope(|s| {
        let handles: Vec<_> = (0..thread_count)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        let image = scanner::build_image(&files[i]);
                        if image.width.is_none() {
                            probe_failures.fetch_add(1, Ordering::Relaxed);
                        }
                        // 探一张落一张：中途退出应用，下次扫描从这里接着走而不是从头再来
                        match app.state::<AppState>().db.lock() {
                            Ok(conn) => {
                                if db::insert_image(&conn, &image).is_err() {
                                    write_failures.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(_) => {
                                write_failures.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        out.push(image);
                        let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = app.emit(
                            "image-scan-progress",
                            ScanProgress { processed: done, total, done: false, warnings: vec![], summary: None },
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
            "{} 张图片未能读取尺寸（请确认已安装 ffmpeg/ffprobe，或文件本身损坏）",
            failures
        ));
    }
    let missed = write_failures.load(Ordering::Relaxed);
    if missed > 0 {
        warnings.push(format!("{} 张图片记录写入数据库失败，下次扫描会重试", missed));
    }
    (images, warnings)
}

/// 批量删除图片：一次回收站事务 + 一次数据库事务，返回成功删除的 id。
/// 逐张删时每张都要过一次 IPC、一次 shell 调用和一次事务落盘，勾选几百张就是十几秒。
#[tauri::command]
pub async fn delete_images(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    image_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    let targets: Vec<(String, String)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(image_ids.len());
        for image_id in &image_ids {
            let path = db::get_image_path(&conn, image_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Image not found: {}", image_id))?;
            out.push((image_id.clone(), path));
        }
        out
    };

    let for_files = targets.clone();
    let survivors = tauri::async_runtime::spawn_blocking(move || undeleted_targets(&for_files))
        .await
        .map_err(|e| e.to_string())?;
    let failed: HashSet<String> = survivors.into_iter().map(|(id, _)| id).collect();
    let deleted: Vec<String> = targets
        .iter()
        .filter(|(id, _)| !failed.contains(id))
        .map(|(id, _)| id.clone())
        .collect();

    if !deleted.is_empty() {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        db::delete_images_by_ids(&tx, &deleted).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        for id in &deleted {
            clear_thumbnail_cache(&app, id);
        }
    }

    Ok(deleted)
}

/// 把图片文件移动到目标文件夹并同步库记录路径；目标重名时自动加 " (2)" 后缀，
/// 写库失败会把文件移回原位
#[tauri::command]
pub async fn move_image(
    state: State<'_, AppState>,
    image_id: String,
    target_dir: String,
) -> Result<String, String> {
    let (old_path, new_path) = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let old_path = db::get_image_path(&conn, &image_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Image not found: {}", image_id))?;
        let src = Path::new(&old_path);
        let filename = src
            .file_name()
            .ok_or_else(|| "源文件路径无效".to_string())?
            .to_string_lossy()
            .to_string();
        let dir = Path::new(&target_dir);
        if dir
            .join(&filename)
            .to_string_lossy()
            .eq_ignore_ascii_case(&old_path)
        {
            return Err("文件已经在该目录中".into());
        }
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
        let ext = src.extension().and_then(|s| s.to_str());
        let mut candidate = dir.join(&filename);
        let mut idx = 1;
        while candidate.exists()
            || db::is_image_path_taken(&conn, &candidate.to_string_lossy(), &image_id)
                .map_err(|e| e.to_string())?
        {
            idx += 1;
            if idx > 999 {
                return Err("目标目录同名文件过多".into());
            }
            let name = match ext {
                Some(e) => format!("{stem} ({idx}).{e}"),
                None => format!("{stem} ({idx})"),
            };
            candidate = dir.join(name);
        }
        (old_path, candidate.to_string_lossy().to_string())
    };

    let (src, dst) = (old_path.clone(), new_path.clone());
    let move_result = tauri::async_runtime::spawn_blocking(move || {
        move_file(Path::new(&src), Path::new(&dst))
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Err(e) = move_result {
        return Err(e);
    }

    if let Err(e) = state
        .db
        .lock()
        .map_err(|e| e.to_string())
        .and_then(|conn| db::update_image_path(&conn, &image_id, &new_path).map_err(|e| e.to_string()))
    {
        let _ = move_file(Path::new(&new_path), Path::new(&old_path));
        return Err(format!("更新库记录失败，已还原文件位置: {}", e));
    }
    Ok(new_path)
}

/// 为指定图片生成缩略图（ffmpeg 等比缩放），网格浏览时不必解码原图
#[tauri::command]
pub async fn generate_image_thumbnails(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    image_ids: Vec<String>,
) -> Result<usize, String> {
    let jobs: Vec<ThumbJob> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let all = db::get_all_images(&conn).map_err(|e| e.to_string())?;
        all.into_iter()
            .filter(|i| image_ids.iter().any(|id| id == &i.id))
            .filter(|i| !cached_thumbnail_usable(&i.thumbnail_path))
            .map(|i| ThumbJob { id: i.id, source: i.path, duration: None })
            .collect()
    };

    run_thumbnail_jobs(&app, jobs, "image-thumbnail-progress", db::set_image_thumbnail).await
}

/// 重复图片的进度事件负载（事件名 duplicate-progress）。
/// 两趟各报一次：`sigs` 是补算缺失指纹，`grouping` 是逐桶补像素并定组。
/// `skipped` 是解不出画面的张数——它们进不了比对，得让界面说清楚而不是报"没有重复"。
/// `groups` 是截至这一次的完整分组（新组只会在定组那一趟里出现，所以跟着整份给）。
#[derive(Clone, serde::Serialize)]
struct DuplicateProgress {
    processed: usize,
    total: usize,
    stage: &'static str,
    skipped: usize,
    groups: Vec<Vec<String>>,
}

/// 检测结论：分组 + 解不出画面而被跳过的张数
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct DuplicateReport {
    pub groups: Vec<Vec<String>>,
    pub skipped: usize,
}

/// 攒着落库的一批双指纹：(image_id, phash, dhash, 指纹对应的文件修改时间)
type SigUpdate = (String, i64, i64, Option<String>);

fn workers(total: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 14)
        .min(total.max(1))
}

/// 抢并行累加器的锁。这些 Mutex 里装的就是个普通集合，毒化只说明别处 panic 过、
/// 内容本身仍然可用；而抢锁时 panic 会顺着 scope 的 join 把整个检测命令带崩——
/// 检测跑几分钟白跑，界面只看到一条"命令失败"。
fn lock_ignoring_poison<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 抢锁落库：抢不到（启动扫描正占着同一把锁）或写失败就原样留着这批，返回有没有写进去。
/// 以前是"抢不到就 clear"，等于这一截 ffmpeg 白跑，下次还得从头算。
fn flush<T>(app: &tauri::AppHandle, batch: &mut Vec<T>, save: impl Fn(&rusqlite::Connection, &[T]) -> rusqlite::Result<()>) -> bool {
    if batch.is_empty() {
        return true;
    }
    match app.state::<AppState>().db.lock() {
        Ok(conn) if save(&conn, batch).is_ok() => {
            batch.clear();
            true
        }
        _ => false,
    }
}

/// 收尾时给没写进去的批次几次重试：扫描的写库是一阵一阵的，等得起
fn flush_with_retry<T>(app: &tauri::AppHandle, batch: &mut Vec<T>, save: impl Fn(&rusqlite::Connection, &[T]) -> rusqlite::Result<()>) {
    for _ in 0..10 {
        if flush(app, batch, &save) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

// 缓存的文件名、原子落盘、读不回来当没有：这三件事三趟检测共用一套，实现在 commands.rs。

/// 缓存里记着"存这份结果时库里有多少条记录"，数量对不上就说明库变了，界面据此提示过期
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarCache {
    pub threshold: u32,
    pub library_count: usize,
    pub result: SimilarResult,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateCache {
    pub library_count: usize,
    pub report: DuplicateReport,
}

#[tauri::command]
pub async fn get_similar_cache(app: tauri::AppHandle) -> Result<Option<SimilarCache>, String> {
    read_cache(&app, SIMILAR_CACHE).await
}

#[tauri::command]
pub async fn get_duplicate_cache(app: tauri::AppHandle) -> Result<Option<DuplicateCache>, String> {
    read_cache(&app, DUPLICATE_CACHE).await
}

/// 结果面板上按 ✕ 是"我不认这份结果"：缓存得跟着删，不然下次打开又给恢复回来
#[tauri::command]
pub async fn clear_detection_cache(app: tauri::AppHandle, which: String) -> Result<(), String> {
    let name = match which.as_str() {
        "similar" => SIMILAR_CACHE,
        "duplicate" => DUPLICATE_CACHE,
        "videoDuplicate" => VIDEO_DUPLICATE_CACHE,
        other => return Err(format!("未知的检测结果：{other}")),
    };
    remove_cache(&app, name);
    Ok(())
}

/// 只对还缺双指纹的行跑 ffmpeg，攒一批落一次库并推一次进度。
/// 逐张 spawn ffmpeg 是这趟的全部代价，所以按线程分片（和相似检测同一套 14 线程上限）。
/// 返回解不出图的张数。
fn ensure_sigs(
    app: &tauri::AppHandle,
    rows: &[db::ImageSig],
    indexes: &[usize],
    pairs: &mut [Option<(u64, u64)>],
) -> usize {
    const EMIT_EVERY: usize = 256;
    const FLUSH_EVERY: usize = 64;
    let total = indexes.len();
    let _ = app.emit("duplicate-progress", DuplicateProgress { processed: 0, total, stage: "sigs", skipped: 0, groups: Vec::new() });
    if total == 0 {
        return 0;
    }

    let mut hits = std::sync::Mutex::new(Vec::<(usize, u64, u64)>::new());
    let mut pending = std::sync::Mutex::new(Vec::<SigUpdate>::new());
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let threads = workers(total);

    std::thread::scope(|s| {
        for slot in 0..threads {
            let (hits, pending, done, failed) = (&hits, &pending, &done, &failed);
            s.spawn(move || {
                let mut batch: Vec<SigUpdate> = Vec::with_capacity(FLUSH_EVERY);
                for (n, &index) in indexes.iter().enumerate() {
                    if n % threads != slot {
                        continue;
                    }
                    let row = &rows[index];
                    match scanner::image_hashes(&row.path) {
                        Some((phash, dhash)) => {
                            lock_ignoring_poison(&hits).push((index, phash, dhash));
                            batch.push((row.id.clone(), phash as i64, dhash as i64, row.modified_at.clone()));
                        }
                        None => {
                            failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    if batch.len() >= FLUSH_EVERY {
                        let _ = flush(app, &mut batch, |conn, rows| db::save_image_sigs(conn, rows));
                    }
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if processed % EMIT_EVERY == 0 || processed == total {
                        let _ = app.emit(
                            "duplicate-progress",
                            DuplicateProgress { processed, total, stage: "sigs", skipped: failed.load(Ordering::Relaxed), groups: Vec::new() },
                        );
                    }
                }
                if !batch.is_empty() {
                    lock_ignoring_poison(&pending).append(&mut batch);
                }
            });
        }
    });

    for (index, phash, dhash) in std::mem::take(hits.get_mut().unwrap_or_else(|e| e.into_inner())) {
        pairs[index] = Some((phash, dhash));
    }
    let mut leftover = std::mem::take(pending.get_mut().unwrap_or_else(|e| e.into_inner()));
    flush_with_retry(app, &mut leftover, |conn, rows| db::save_image_sigs(conn, rows));
    failed.into_inner()
}

/// 第二趟：逐桶补 32×32 缩略像素（顺手落库，下次不必再解）并定组。
/// 两件事合在一趟做，是因为桶之间本来就互不相干——按桶分片既能并行，
/// 也意味着每定下一个桶就能推一次当前完整分组，界面边跑边看已经定下来的组。
/// `already_skipped` 是第一趟解不出画面的张数，进度里报的是两趟累计。
fn group_pass(
    app: &tauri::AppHandle,
    rows: &[db::ImageSig],
    buckets: Vec<Vec<usize>>,
    already_skipped: usize,
) -> (Vec<Vec<String>>, usize) {
    const EMIT_EVERY: usize = 16;
    const FLUSH_EVERY: usize = 32;
    let total = buckets.len();
    let skipped = already_skipped;
    let _ = app.emit(
        "duplicate-progress",
        DuplicateProgress { processed: 0, total, stage: "grouping", skipped, groups: Vec::new() },
    );
    if total == 0 {
        return (Vec::new(), skipped);
    }

    let mut collected = std::sync::Mutex::new(Vec::<Vec<String>>::new());
    let mut pending = std::sync::Mutex::new(Vec::<(String, Vec<u8>)>::new());
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let threads = workers(total);
    let shares = buckets.as_slice();

    std::thread::scope(|s| {
        for slot in 0..threads {
            let (collected, pending, done, failed) = (&collected, &pending, &done, &failed);
            s.spawn(move || {
                let mut batch: Vec<(String, Vec<u8>)> = Vec::with_capacity(FLUSH_EVERY);
                for (n, indexes) in shares.iter().enumerate() {
                    if n % threads != slot {
                        continue;
                    }
                    let mut pool: Vec<Candidate> = Vec::with_capacity(indexes.len());
                    for &index in indexes {
                        let row = &rows[index];
                        // 解不出画面的条目只能跳过：宁可漏一组，也不拿猜的当重复
                        let grid = match row.cached_pixels().map(<[u8]>::to_vec) {
                            Some(hit) => hit,
                            None => match scanner::gray_pixels(&row.path, PIX_GRID) {
                                Some(fresh) => {
                                    if row.sig_pixels.is_none() {
                                        batch.push((row.id.clone(), fresh.clone()));
                                    }
                                    fresh
                                }
                                None => {
                                    failed.fetch_add(1, Ordering::Relaxed);
                                    continue;
                                }
                            },
                        };
                        pool.push(Candidate {
                            index,
                            id: row.id.clone(),
                            created_at: row.created_at.clone(),
                            pixels: grid,
                        });
                    }
                    if pool.len() > 1 {
                        let fresh = group_bucket(rows, pool);
                        if !fresh.is_empty() {
                            lock_ignoring_poison(&collected).extend(fresh);
                        }
                    }
                    if batch.len() >= FLUSH_EVERY {
                        let _ = flush(app, &mut batch, |conn, rows| db::save_image_pixels(conn, rows));
                    }
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if processed % EMIT_EVERY == 0 || processed == total {
                        let groups = lock_ignoring_poison(&collected).clone();
                        let _ = app.emit(
                            "duplicate-progress",
                            DuplicateProgress {
                                processed,
                                total,
                                stage: "grouping",
                                skipped: skipped + failed.load(Ordering::Relaxed),
                                groups,
                            },
                        );
                    }
                }
                if !batch.is_empty() {
                    lock_ignoring_poison(&pending).append(&mut batch);
                }
            });
        }
    });

    let mut leftover = std::mem::take(pending.get_mut().unwrap_or_else(|e| e.into_inner()));
    flush_with_retry(app, &mut leftover, |conn, rows| db::save_image_pixels(conn, rows));
    let mut groups = std::mem::take(collected.get_mut().unwrap_or_else(|e| e.into_inner()));
    groups.sort_by_key(|group| std::cmp::Reverse(group.len()));
    (groups, skipped + failed.into_inner())
}

/// 存库当缓存的那一档边长
const PIX_GRID: usize = 32;
/// 判同门槛：整幅平均差 ≤ 2/255，且差过 16 级的像素不到 10%。
/// 门槛是量出来的：本机 2400 多个"双指纹完全相同"的候选桶里，p50 = 0.2、p90 = 0.74，
/// 也就是重存/转格式的抖动基本都在 1 以内；两张不同的图随便就差到十几，这条线留着一个数量级。
const PIX_MEAN_MAX: f64 = 2.0;
const PIX_BIG_SHARE: f64 = 0.10;
/// 32 档对不上时再试的两档。两张分辨率差得远的图缩到某个固定网格会撞上采样干涉——
/// 实测同一张图在 32/64/96 档差 8 级、在 16/128/192 档只差 0.5 级，多试一档就少漏一批。
/// 只有前一档对不上的候选才会走到这儿，所以代价只落在极少数条目上。
const PIX_RESCUE_GRIDS: [usize; 2] = [96, 192];
/// 32 档差过这个线就不再去试高分辨率档：真副本被采样干涉拉开也就到 8 级上下，
/// 十几级开外的是平图/连环截图撞哈希，换档位也救不回来，而每一档都是一次 ffmpeg。
const RESCUE_MEAN_MAX: f64 = 12.0;

/// 两幅同档灰度的（平均每级差，差过 16 级的像素占比）
fn pixel_gap(a: &[u8], b: &[u8]) -> Option<(f64, f64)> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut sum = 0u64;
    let mut big = 0usize;
    for (x, y) in a.iter().zip(b) {
        let diff = (*x as i16 - *y as i16).abs();
        sum += diff as u64;
        if diff > 16 {
            big += 1;
        }
    }
    Some((sum as f64 / a.len() as f64, big as f64 / a.len() as f64))
}

/// 两幅缩略像素算不算同一张图
fn pixels_close(a: &[u8], b: &[u8]) -> bool {
    match pixel_gap(a, b) {
        Some((mean, share)) => mean <= PIX_MEAN_MAX && share <= PIX_BIG_SHARE,
        None => false,
    }
}

/// 桶里的一个候选
struct Candidate {
    /// 在 rows 里的下标（现算高分辨率档时按它取路径）
    index: usize,
    id: String,
    created_at: String,
    pixels: Vec<u8>,
}

/// 按需现算一档高分辨率像素，同一个桶里不重复解码
fn rescue_grid(
    rows: &[db::ImageSig],
    cache: &mut HashMap<(usize, usize), Vec<u8>>,
    index: usize,
    n: usize,
) -> Option<Vec<u8>> {
    if let Some(hit) = cache.get(&(index, n)) {
        return Some(hit.clone());
    }
    let pixels = scanner::gray_pixels(&rows[index].path, n)?;
    cache.insert((index, n), pixels.clone());
    Some(pixels)
}

fn close_against(
    rows: &[db::ImageSig],
    cache: &mut HashMap<(usize, usize), Vec<u8>>,
    a: &Candidate,
    b: &Candidate,
) -> bool {
    if pixels_close(&a.pixels, &b.pixels) {
        return true;
    }
    // 差得太远的对不配花两次 ffmpeg：这一趟候选量是老口径的百倍，省掉的都是白省
    if pixel_gap(&a.pixels, &b.pixels).map_or(true, |(mean, _)| mean > RESCUE_MEAN_MAX) {
        return false;
    }
    for &n in &PIX_RESCUE_GRIDS {
        let (Some(x), Some(y)) = (
            rescue_grid(rows, cache, a.index, n),
            rescue_grid(rows, cache, b.index, n),
        ) else {
            continue;
        };
        if pixels_close(&x, &y) {
            return true;
        }
    }
    false
}

/// 一个桶内定组：按入库时间升序，最早那张当保留原件，其余逐张跟它核对——
/// 只对得上的进组，对不下的留给下一轮另起一组（所以一组内任意两张都是直接比过的）。
fn group_bucket(rows: &[db::ImageSig], mut pool: Vec<Candidate>) -> Vec<Vec<String>> {
    pool.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    // 缓存按池开：一个池通常三五张，犯不着把整库的高清像素攒在手里
    let mut cache: HashMap<(usize, usize), Vec<u8>> = HashMap::new();
    let mut groups: Vec<Vec<String>> = Vec::new();
    while !pool.is_empty() {
        let keeper = pool.remove(0);
        let mut group = vec![keeper.id.clone()];
        pool.retain(|other| {
            let matched = close_against(rows, &mut cache, &keeper, other);
            if matched {
                group.push(other.id.clone());
            }
            !matched
        });
        if group.len() > 1 {
            groups.push(group);
        }
    }
    groups
}

/// 候选门槛：两枚感知哈希各差 ≤ DUP_GATE 位就要拿到像素层去对，收不收由像素说了算。
/// 老口径是"两枚全等"，实库 18.3 万枚指纹只圈出 37 对、像素层真收下的只有 9 对
/// （另 28 对是平图撞车）——重存一遍就有 1~2 位抖动，真副本因此大批留在相似组里
/// 进不了重复判定（用户报的正是这个）。4 是量出来的拐点，各档"候选对/像素收下"：
/// 1→1959/1622、2→1221/792、3→783/359、4→601/222，再往上收下率掉到两成五以下
/// （5→470/114、6→382/84），多出来的基本都是撞车。
/// 代价：候选从 74 张涨到 8,209 张，其中约 6.8 千张要现解 32 档像素（本机实测九分半，
/// 解完就落库，第二次跑只剩零头）。
const DUP_GATE: u32 = 4;

/// 用两枚感知哈希圈候选池：先做一次全量两两比对（和相似检测同一套按行分片，
/// 18.3 万枚实测 13 秒），再把"两路距离都 ≤ 门槛"的边并成池。
/// 池只是候选名单，最后仍由缩略像素定组——所以这里宁松勿紧，但不放过就等于不判。
fn candidate_pools(pairs: &[Option<(u64, u64)>], gate: u32) -> Vec<Vec<usize>> {
    let reps: Vec<(usize, u64, u64)> = pairs
        .iter()
        .enumerate()
        .filter_map(|(index, pair)| pair.map(|(phash, dhash)| (index, phash, dhash)))
        .collect();
    let total = reps.len();
    let edges: Vec<(u32, u32)> = std::thread::scope(|s| {
        let threads = workers(total);
        let mut handles = Vec::with_capacity(threads);
        for slot in 0..threads {
            let reps = reps.as_slice();
            handles.push(s.spawn(move || {
                let mut local: Vec<(u32, u32)> = Vec::new();
                let mut i = slot;
                while i < total {
                    let (pi, di) = (reps[i].1, reps[i].2);
                    for j in 0..i {
                        let dd = (pi ^ reps[j].1).count_ones().max((di ^ reps[j].2).count_ones());
                        if dd <= gate {
                            local.push((i as u32, j as u32));
                        }
                    }
                    i += threads;
                }
                local
            }));
        }
        // scope 退出时本来就会把子线程的 panic 再抛一遍，这里的 unwrap 只是照实取值；
        // 子线程里唯一的 panic 来源（抢锁）已经在 lock_ignoring_poison 里堵掉了
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .flatten()
            .collect()
    });

    let mut uf = UnionFind::new(total);
    for (a, b) in edges {
        uf.union(a as usize, b as usize);
    }
    let mut pools: HashMap<usize, Vec<usize>> = HashMap::new();
    for rep in 0..total {
        pools.entry(uf.find(rep)).or_default().push(reps[rep].0);
    }
    pools.into_values().filter(|pool| pool.len() > 1).collect()
}

/// 找出内容相同的重复图片组：不再比字节（重存一遍就漏，读全盘也慢），
/// 改成"两枚感知哈希圈候选 + 缩略像素定组"——重存/转格式改得动字节，改不动画面。
#[tauri::command]
pub async fn find_duplicate_images(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DuplicateReport, String> {
    // 判据换成解码指纹之后，ffmpeg 成了硬依赖（老的字节签名不用解码也能算）
    if !crate::scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法比对图片内容。请安装 ffmpeg 并加入 PATH。".into());
    }
    // 文件已经不在磁盘上的条目不参与判定
    let mut rows: Vec<db::ImageSig> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_image_sigs(&conn).map_err(|e| e.to_string())?
    };
    let library_count = rows.len();
    rows.retain(|row| Path::new(&row.path).exists());

    let report = tauri::async_runtime::spawn_blocking(move || {
        let mut pairs: Vec<Option<(u64, u64)>> = rows.iter().map(|row| row.cached_sigs()).collect();
        let need_sigs: Vec<usize> = (0..rows.len()).filter(|&i| pairs[i].is_none()).collect();
        let skipped = ensure_sigs(&app, &rows, &need_sigs, pairs.as_mut_slice());

        // 哈希差得远的到不了像素层，绝大多数条目在这一步就被排除
        let candidates = candidate_pools(&pairs, DUP_GATE);

        // 补像素和定组合成一趟：每核对完一个候选池就推一次当前分组
        let (groups, skipped) = group_pass(&app, &rows, candidates, skipped);
        let report = DuplicateReport { groups, skipped };
        // 这一趟热跑也要九分钟，落一份缓存，重启后直接接着看
        write_cache(
            &app,
            DUPLICATE_CACHE,
            &DuplicateCache {
                library_count,
                report: report.clone(),
            },
        );
        report
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(report)
}

/// 相似图检测的进度事件负载（事件名 similar-progress）
#[derive(Clone, serde::Serialize)]
pub struct SimilarProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    /// 当前完整分组结果（口径换了组会缩小，所以整份给，前端整份替换）
    pub groups: Vec<Vec<String>>,
    /// 挂在组尾的远亲：有邻居但没连上骨架，可见但不自动勾
    pub far: Vec<String>,
}

/// 一张成组图片的指纹：u64 拆成两个 u32，前端按 JS number 做异或数 1，不必碰 BigInt
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SimilarHit {
    pub id: String,
    pub lo: u32,
    pub hi: u32,
}

/// 检测结论：分组 + 远亲名单 + 组内各成员的指纹。带上指纹，面板才能按"距保留张多远"排序
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SimilarResult {
    pub groups: Vec<Vec<String>>,
    pub far: Vec<String>,
    pub hashes: Vec<SimilarHit>,
}

/// 一条边要"两端互为前 NEAR_RANK 名近邻"才算骨架边。
/// 实测 18.6 万张真指纹：传递闭包在阈值 14 起就渗透（16 时串出一个 33401 张的组，
/// 占全部成组图的 67%），k=2 之后最大组 15 张，且阈值 10 那批真副本 99.4% 仍在组内。
/// k=3 会漏回大组（最大 2778），所以这个 2 是量出来的拐点，不是猜的。
const NEAR_RANK: usize = 2;

/// 远亲找归属时最多沿"更好的邻居"走几步（链太长说明本来就谁也不像谁）
const FAR_HOPS: usize = 4;

#[derive(Clone, Copy)]
struct SigItem<'a> {
    id: &'a str,
    created: &'a str,
    phash: u64,
    dhash: u64,
}

/// 分组结果：成员是图片下标，组内按加入时间升序；far 里的是挂来的远亲
struct SimilarPartition {
    groups: Vec<Vec<usize>>,
    far: HashSet<usize>,
}

const NO_GROUP: usize = usize::MAX;

/// 双指纹比对图。节点是"代表"（两枚指纹完全相同的只留一个，同图必同组），
/// 边是"两路汉明距离都 ≤ 阈值"，成组看的是互为前 2 近邻的骨架边。
struct SimilarGraph<'a> {
    threshold: u32,
    items: Vec<SigItem<'a>>,
    reps: Vec<usize>,
    rep_of: HashMap<(u64, u64), usize>,
    /// 代表 → 它承载的全部图片下标（含它自己）
    twins: Vec<Vec<usize>>,
    /// 代表 → 按 (距离, 另一端) 排好序的邻居表，只收 ≤ 阈值的
    nbrs: Vec<Vec<(u32, u32)>>,
    /// 已经比对过的代表数，用于增量补边
    scanned: usize,
}

impl<'a> SimilarGraph<'a> {
    fn new(threshold: u32) -> Self {
        SimilarGraph {
            threshold,
            items: Vec::new(),
            reps: Vec::new(),
            rep_of: HashMap::new(),
            twins: Vec::new(),
            nbrs: Vec::new(),
            scanned: 0,
        }
    }

    /// 收一张图。同指纹的直接并到那张代表的组里，一次比对都不用
    fn add(&mut self, id: &'a str, created: &'a str, phash: u64, dhash: u64) {
        let item = self.items.len();
        self.items.push(SigItem { id, created, phash, dhash });
        match self.rep_of.get(&(phash, dhash)).copied() {
            Some(rep) => self.twins[rep].push(item),
            None => {
                self.rep_of.insert((phash, dhash), self.reps.len());
                self.reps.push(item);
                self.twins.push(vec![item]);
                self.nbrs.push(Vec::new());
            }
        }
    }

    fn hash_of(&self, rep: usize) -> (u64, u64) {
        let item = self.items[self.reps[rep]];
        (item.phash, item.dhash)
    }

    /// 给还没比对过的那批代表补边。每行 i 只和 j<i 比，所以一对只算一次；
    /// 行与行之间互不依赖，按线程分片。全库 18.6 万枚实测 11.6 秒（14 线程）。
    fn scan(&mut self) {
        let from = self.scanned;
        let total = self.reps.len();
        if from >= total {
            return;
        }
        let hashes: Vec<(u64, u64)> = (0..total).map(|rep| self.hash_of(rep)).collect();
        let hashes = hashes.as_slice();
        let threshold = self.threshold;
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(2, 14);
        let found: Vec<Vec<(u32, u32, u32)>> = std::thread::scope(|s| {
            let mut handles = Vec::with_capacity(threads);
            for ti in 0..threads {
                handles.push(s.spawn(move || {
                    let mut local: Vec<(u32, u32, u32)> = Vec::new();
                    let mut i = from + ti;
                    while i < total {
                        let (pi, di) = hashes[i];
                        for j in 0..i {
                            let (pj, dj) = hashes[j];
                            let dd = (pi ^ pj).count_ones().max((di ^ dj).count_ones());
                            if dd <= threshold {
                                local.push((i as u32, j as u32, dd));
                            }
                        }
                        i += threads;
                    }
                    local
                }));
            }
            // 同上：子线程只会算一段边表，没有会 panic 的操作，scope 也兜着这一层
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let mut touched: HashSet<usize> = HashSet::new();
        for batch in found {
            for (i, j, dd) in batch {
                self.nbrs[i as usize].push((dd, j));
                self.nbrs[j as usize].push((dd, i));
                touched.insert(i as usize);
                touched.insert(j as usize);
            }
        }
        // 新边会落到老行的头上，所以按"被碰过"重排，不能只排新行
        for rep in touched {
            let row = &mut self.nbrs[rep];
            row.sort_unstable();
            row.dedup_by(|a, b| a.1 == b.1);
        }
        self.scanned = total;
    }

    /// 这条边算不算骨架：两端都把它排进自己的前 NEAR_RANK 名
    fn mutual(&self, i: usize, j: u32) -> bool {
        self.nbrs[i].iter().take(NEAR_RANK).any(|&(_, other)| other == j)
            && self.nbrs[j as usize].iter().take(NEAR_RANK).any(|&(_, other)| other as usize == i)
    }

    fn partition(&self) -> SimilarPartition {
        let total = self.reps.len();
        let mut uf = UnionFind::new(total);
        for i in 0..total {
            for &(_, j) in self.nbrs[i].iter().take(NEAR_RANK) {
                if (j as usize) > i && self.mutual(i, j) {
                    uf.union(i, j as usize);
                }
            }
        }

        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for rep in 0..total {
            members.entry(uf.find(rep)).or_default().push(rep);
        }
        // 骨架组：多于一枚代表，或者一枚代表身上背着孪生张（同图，肯定算一组）
        let mut comps: Vec<Vec<usize>> = members
            .into_values()
            .filter(|reps| reps.len() > 1 || self.twins[reps[0]].len() > 1)
            .collect();
        comps.sort_by(|a, b| a[0].cmp(&b[0]));
        let mut group_of = vec![NO_GROUP; total];
        for (index, reps) in comps.iter().enumerate() {
            for &rep in reps {
                group_of[rep] = index;
            }
        }

        // 落单的：沿"邻居里名次最好的、还没走过的"往上有归属的方向挂，挂上就是远亲
        let mut far: Vec<Vec<usize>> = vec![Vec::new(); comps.len()];
        let mut lonely: Vec<usize> = (0..total).filter(|&rep| group_of[rep] == NO_GROUP).collect();
        lonely.sort_unstable();
        for rep in lonely {
            if let Some(host) = self.find_host(rep, &group_of) {
                far[host].push(rep);
            }
        }

        let mut groups: Vec<Vec<usize>> = Vec::with_capacity(comps.len());
        let mut far_set: HashSet<usize> = HashSet::new();
        for (index, reps) in comps.iter().enumerate() {
            let mut items: Vec<usize> = reps.iter().flat_map(|&rep| self.twins[rep].iter().copied()).collect();
            for &guest in &far[index] {
                items.extend(self.twins[guest].iter().copied());
                far_set.extend(self.twins[guest].iter().copied());
            }
            items.sort_by_key(|&item| (self.items[item].created, self.items[item].id));
            groups.push(items);
        }
        groups.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a[0].cmp(&b[0])));
        SimilarPartition { groups, far: far_set }
    }

    /// 这只落单代表该挂到哪一组：先看自己的邻居，够不着就沿名次更好的一方继续走几步。
    /// 走过的节点用一个小数组记着就够了（最多 FAR_HOPS+1 个），整库位图会为一张图清 18 万格。
    fn find_host(&self, rep: usize, group_of: &[usize]) -> Option<usize> {
        if self.nbrs[rep].is_empty() {
            return None;
        }
        let mut walked: Vec<usize> = Vec::with_capacity(FAR_HOPS + 1);
        let mut cur = rep;
        for _ in 0..=FAR_HOPS {
            walked.push(cur);
            for &(_, next) in &self.nbrs[cur] {
                let host = group_of[next as usize];
                if host != NO_GROUP {
                    return Some(host);
                }
            }
            match self.nbrs[cur].iter().map(|&(_, n)| n as usize).find(|n| !walked.contains(n)) {
                Some(next) => cur = next,
                None => return None,
            }
        }
        None
    }

    fn member_ids(&self, partition: &SimilarPartition) -> Vec<Vec<String>> {
        partition
            .groups
            .iter()
            .map(|items| items.iter().map(|&item| self.items[item].id.to_string()).collect())
            .collect()
    }

    fn far_ids(&self, partition: &SimilarPartition) -> Vec<String> {
        let mut ids: Vec<usize> = partition.far.iter().copied().collect();
        ids.sort_unstable();
        ids.into_iter().map(|item| self.items[item].id.to_string()).collect()
    }

    /// 成组图片的指纹（孤张不给，免得 19 万条清单白传一趟）
    fn signatures(&self, partition: &SimilarPartition) -> Vec<SimilarHit> {
        let mut out = Vec::new();
        for items in &partition.groups {
            for &item in items {
                let hash = self.items[item].phash;
                out.push(SimilarHit {
                    id: self.items[item].id.to_string(),
                    lo: hash as u32,
                    hi: (hash >> 32) as u32,
                });
            }
        }
        out
    }
}

/// 并查集（按规模合并，路径压缩）
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind { parent: (0..n).collect(), size: vec![1; n] }
    }
    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut cur = x;
        while self.parent[cur] != cur {
            let next = self.parent[cur];
            self.parent[cur] = root;
            cur = next;
        }
        root
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra] >= self.size[rb] { (ra, rb) } else { (rb, ra) };
        self.parent[small] = big;
        self.size[big] += self.size[small];
    }
}

/// 推一次当前完整分组给前端；done 时不带成员（完整结果走命令返回值）
fn emit_similar_progress(
    app: &tauri::AppHandle,
    processed: usize,
    total: usize,
    done: bool,
    graph: &SimilarGraph,
    partition: &SimilarPartition,
) {
    let (groups, far) = if done { (Vec::new(), Vec::new()) } else { (graph.member_ids(partition), graph.far_ids(partition)) };
    let _ = app.emit(
        "similar-progress",
        SimilarProgress { processed, total, done, groups, far },
    );
}

/// 找出"相似但不相同"的图片组（连拍/截图系列）：pHash + dHash 双指纹，两路都要 ≤ threshold。
/// 与 find_duplicate_images 互补：字节级去重只认完全相同，这里抓视觉近似。
/// 成组看的不是"有一条链连着"，而是"两端互为前 2 近邻"——闭包会把一片撞车的截图串成 3 万张一组。
/// 指纹算过一次就落在库里，只有新图/改过的图才再跑 ffmpeg；已缓存的部分先聚好推出去，
/// 剩下的边算边推（每推一次都是当前的完整分组，前端整份替换），命令返回值是最终结果 + 组成员指纹。
#[tauri::command]
pub async fn find_similar_images(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    threshold: u32,
) -> Result<SimilarResult, String> {
    /// 每补算这么多张落一次指纹库
    const FLUSH_EVERY: usize = 256;
    /// 两次推送之间至少隔这么久：完整结果上百 KB，一秒推几回纯浪费
    const EMIT_AT_LEAST: std::time::Duration = std::time::Duration::from_millis(2000);
    let threshold = threshold.clamp(1, 32);

    if !crate::scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法计算图片指纹。请安装 ffmpeg 并加入 PATH。".into());
    }

    // 文件已经不在磁盘上的条目不参与聚类，也不用再白跑一遍解码
    let mut rows: Vec<db::ImageSig> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_image_sigs(&conn).map_err(|e| e.to_string())?
    };
    // 缓存的过期判断按"库里的记录数"，所以要在过滤之前数
    let library_count = rows.len();
    rows.retain(|row| Path::new(&row.path).exists());

    let result = tauri::async_runtime::spawn_blocking(move || -> SimilarResult {
        let mut graph = SimilarGraph::new(threshold);
        let mut pending: Vec<&db::ImageSig> = Vec::new();
        for row in &rows {
            match row.cached_sigs() {
                Some((phash, dhash)) => graph.add(row.id.as_str(), row.created_at.as_str(), phash, dhash),
                None => pending.push(row),
            }
        }
        let total = pending.len();
        // 缓存里已有的那部分不花钱：先把已经能看的组推出去，面板不用等补算完
        graph.scan();
        let mut partition = graph.partition();
        let mut last_emit = std::time::Instant::now();
        emit_similar_progress(&app, 0, total, false, &graph, &partition);

        let mut processed = 0usize;
        let mut fresh: Vec<(String, i64, i64, Option<String>)> = Vec::with_capacity(FLUSH_EVERY);
        for row in pending {
            if let Some((phash, dhash)) = scanner::image_hashes(&row.path) {
                graph.add(row.id.as_str(), row.created_at.as_str(), phash, dhash);
                fresh.push((row.id.clone(), phash as i64, dhash as i64, row.modified_at.clone()));
            }
            processed += 1;
            if processed % FLUSH_EVERY == 0 || processed == total {
                if let Ok(conn) = app.state::<AppState>().db.lock() {
                    let _ = db::save_image_sigs(&conn, &fresh);
                }
                fresh.clear();
                graph.scan();
                if last_emit.elapsed() >= EMIT_AT_LEAST || processed == total {
                    partition = graph.partition();
                    emit_similar_progress(&app, processed, total, false, &graph, &partition);
                    last_emit = std::time::Instant::now();
                }
            }
        }
        if let Ok(conn) = app.state::<AppState>().db.lock() {
            let _ = db::save_image_sigs(&conn, &fresh);
        }

        graph.scan();
        partition = graph.partition();
        emit_similar_progress(&app, processed, total, true, &graph, &partition);
        let result = SimilarResult {
            groups: graph.member_ids(&partition),
            far: graph.far_ids(&partition),
            hashes: graph.signatures(&partition),
        };
        // 落一份缓存：重启应用后打开面板直接就是这些组，不必再等一趟
        write_cache(
            &app,
            SIMILAR_CACHE,
            &SimilarCache {
                threshold,
                library_count,
                result: result.clone(),
            },
        );
        result
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{sample_image, setup_test_db};
    use std::fs;

    fn grid(value: u8) -> Vec<u8> {
        vec![value; PIX_GRID * PIX_GRID]
    }

    fn candidate(index: usize, id: &str, created_at: &str, shift: i16) -> Candidate {
        let pixels = grid(100).iter().map(|&v| (v as i16 + shift) as u8).collect();
        Candidate {
            index,
            id: id.to_string(),
            created_at: created_at.to_string(),
            pixels,
        }
    }

    /// 只给按需解码取路径用：这些路径不存在，高分辨率档拿不到像素，判定就停在 32 档
    fn ghost_rows(n: usize) -> Vec<db::ImageSig> {
        (0..n)
            .map(|i| db::ImageSig {
                id: format!("i{i}"),
                path: format!("C:/missing/{i}.png"),
                created_at: "2026-01-01".to_string(),
                modified_at: None,
                phash: None,
                dhash: None,
                sig_pixels: None,
                sig_modified_at: None,
            })
            .collect()
    }

    #[test]
    fn test_pixels_close_tolerates_resave_noise() {
        let base = grid(100);
        // 重存/转格式的抖动是逐像素一两级的噪声：整幅平均差远小于门槛，仍算同一张
        let resaved: Vec<u8> = base.iter().map(|&v| v + 1).collect();
        assert!(pixels_close(&base, &resaved));
        // 两张不同的图：明暗差一截，谁都过不去
        assert!(!pixels_close(&base, &grid(140)));
    }

    #[test]
    fn test_pixels_close_rejects_a_local_edit() {
        let base = grid(100);
        // 截图上改了一小块（多一行字）：八分之一的像素强差异，够把它挡在重复之外
        let edited: Vec<u8> = base
            .iter()
            .enumerate()
            .map(|(i, &v)| if i < base.len() / 8 { v + 60 } else { v })
            .collect();
        assert!(!pixels_close(&base, &edited));
        // 同样的块、差异小于一级的门槛：肉眼看不出区别，算重复
        let faint: Vec<u8> = base
            .iter()
            .enumerate()
            .map(|(i, &v)| if i < base.len() / 8 { v + 2 } else { v })
            .collect();
        assert!(pixels_close(&base, &faint));
    }

    #[test]
    fn test_pixels_close_rejects_mismatched_grids() {
        // 两边长度都不等（换了采样口径/写坏的缓存）时不比对，直接算不像
        assert!(!pixels_close(&grid(10), &vec![10; 4]));
        assert!(!pixels_close(&[], &[]));
    }

    #[test]
    fn test_candidate_pools_admit_a_few_bits_of_drift() {
        let noisy = (1u64 << 40) - 1;
        let pairs = vec![
            Some((0, 0)),
            Some((1, 2)),
            Some((noisy, noisy)),
            None,
            Some((1, 0)),
        ];
        // 差 1~2 位的副本要能进同一个池（老口径"两枚全等"在这儿会把真副本挡在门外），
        // 差 40 位的自己待着，缺指纹的压根不参与
        assert_eq!(candidate_pools(&pairs, DUP_GATE), vec![vec![0, 1, 4]]);
        // 门槛收回 0 位时又只剩完全相同的了
        assert!(candidate_pools(&pairs, 0).is_empty());
    }

    #[test]
    fn test_group_bucket_keeps_the_earliest_added_as_keeper() {
        let rows = ghost_rows(3);
        let pool = vec![
            candidate(0, "late", "2026-01-03", 0),
            candidate(1, "early", "2026-01-01", 1),
            candidate(2, "middle", "2026-01-02", 0),
        ];
        // 首张是入库最早的那张，其余按入库时间依次跟它核对
        assert_eq!(
            group_bucket(&rows, pool),
            vec![vec!["early".to_string(), "middle".to_string(), "late".to_string()]]
        );
    }

    #[test]
    fn test_group_bucket_does_not_inherit_chain_similarity() {
        let rows = ghost_rows(3);
        // A(100) ~ B(102) ~ C(104)：A 与 C 均值差 4 已超门槛，不能靠 B 串成一组
        let pool = vec![
            candidate(0, "a", "2026-01-01", 0),
            candidate(1, "b", "2026-01-02", 2),
            candidate(2, "c", "2026-01-03", 4),
        ];
        assert_eq!(group_bucket(&rows, pool), vec![vec!["a".to_string(), "b".to_string()]]);
    }

    #[test]
    fn test_group_bucket_splits_a_bucket_into_two_groups() {
        let rows = ghost_rows(4);
        // 同一个哈希桶里混着两拨：一幅整体偏亮的和一幅偏暗的，各自成组
        let pool = vec![
            candidate(0, "bright1", "2026-01-01", 0),
            candidate(1, "dark1", "2026-01-02", 40),
            candidate(2, "bright2", "2026-01-03", 1),
            candidate(3, "dark2", "2026-01-04", 41),
        ];
        let groups = group_bucket(&rows, pool);
        assert_eq!(
            groups,
            vec![
                vec!["bright1".to_string(), "bright2".to_string()],
                vec!["dark1".to_string(), "dark2".to_string()],
            ]
        );
    }

    /// 同一幅图换格式、换分辨率之后仍要被认成重复——这条是判据改写的全部意义
    #[test]
    fn test_resaved_and_rescaled_picture_stays_close() {
        if !scanner::ffmpeg_available() {
            eprintln!("跳过：本机未安装 ffmpeg");
            return;
        }
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("viewman_pixels_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let render = |name: &str, extra: &[&str]| {
            let out = dir.join(name);
            let ok = Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=1", "-frames:v", "1"])
                .args(extra)
                .arg(&out)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "生成测试图片失败");
            out.to_string_lossy().to_string()
        };
        let original = render("orig.png", &[]);
        // 同一幅画面重存成 jpg，再放大一倍：字节面目全非，画面还是那幅画面
        let resaved = render("resaved.jpg", &["-q:v", "3"]);
        let upscaled = render("upscaled.png", &["-vf", "scale=1280:960:flags=bicubic"]);
        // 另一幅内容完全不同的图，用来确认门槛不是"什么都算重复"
        let other = {
            let out = dir.join("other.png");
            assert!(Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", "rgbtestsrc=size=320x240:rate=10:duration=1", "-frames:v", "1"])
                .arg(&out)
                .output()
                .unwrap()
                .status
                .success());
            out.to_string_lossy().to_string()
        };

        let base = scanner::gray_pixels(&original, PIX_GRID).unwrap();
        for path in [&resaved, &upscaled] {
            let pixels = scanner::gray_pixels(path, PIX_GRID).unwrap();
            assert!(pixels_close(&base, &pixels), "{path} 应与原图判为同一张");
        }
        assert!(
            !pixels_close(&base, &scanner::gray_pixels(&other, PIX_GRID).unwrap()),
            "内容不同的两幅图不该判为重复"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// 一把灌完再聚：返回值是（分组，远亲名单），跟面板看到的一样
    fn clustered(threshold: u32, items: &[(&str, &str, u64, u64)]) -> (Vec<Vec<String>>, Vec<String>) {
        let mut graph = SimilarGraph::new(threshold);
        for &(id, created, phash, dhash) in items {
            graph.add(id, created, phash, dhash);
        }
        graph.scan();
        let partition = graph.partition();
        (graph.member_ids(&partition), graph.far_ids(&partition))
    }

    #[test]
    fn test_similar_graph_groups_a_short_chain_and_drops_a_lonely_one() {
        let (groups, far) = clustered(
            3,
            &[
                ("a", "2026-01-01", 0b0000u64, 0),
                ("b", "2026-01-01", 0b0011, 0),
                ("c", "2026-01-01", 0b1111, 0),
                ("d", "2026-01-01", 0xFFFF_FFFF_FFFF_FFF0, 0),
            ],
        );
        // a~b、b~c 都 ≤3，各自也只有这一个邻居：三张照旧成一组，谁也不像的 d 连组都进不了
        assert_eq!(groups, vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]]);
        assert!(far.is_empty());
    }

    /// 这条就是 3.3 万张那组病根的解药：人人都把中心排进自己第一近邻，闭包会把一整片
    /// 撞车的截图串成一组；现在中心只认名次最好的两个，其余当远亲挂在尾巴上。
    #[test]
    fn test_similar_graph_keeps_only_two_skeleton_leaves_per_hub() {
        let (groups, far) = clustered(
            3,
            &[
                ("hub", "2026-01-01", 0u64, 0),
                ("n1", "2026-01-01", 0b0000_0011, 0),
                ("n2", "2026-01-01", 0b0000_1100, 0),
                ("n3", "2026-01-01", 0b0011_0000, 0),
                ("n4", "2026-01-01", 0b1100_0000, 0),
                ("n5", "2026-01-01", 0b0000_0000_0000_0011_0000_0000_0000_0000, 0),
            ],
        );
        // 中心与五张各差 2 位、五张彼此差 4 位：骨架边只留前两枚，后三枚挂成远亲
        assert_eq!(
            groups,
            vec![vec!["hub".to_string(), "n1".to_string(), "n2".to_string(),
                "n3".to_string(), "n4".to_string(), "n5".to_string()]]
        );
        assert_eq!(far, vec!["n3".to_string(), "n4".to_string(), "n5".to_string()]);
    }

    #[test]
    fn test_similar_graph_needs_both_fingerprints_to_agree() {
        // 结构（pHash）一模一样，明暗走向（dHash）相差 20 位：套图换内容的那种，不该算相似
        let (groups, far) = clustered(
            3,
            &[
                ("left", "2026-01-01", 0b0000, 0xFFFF_FFFF_0000_000F),
                ("right", "2026-01-02", 0b0000, 0x0000_0000_FFFF_FFF0),
            ],
        );
        assert!(groups.is_empty());
        assert!(far.is_empty());

        // 同一张图重新存了一遍：两枚指纹完全相同，直接算一组
        let (groups, _) = clustered(
            3,
            &[
                ("left", "2026-01-01", 0b0000, 0xFFFF_FFFF_0000_000F),
                ("right", "2026-01-02", 0b0000, 0x0000_0000_FFFF_FFF0),
                ("again", "2026-01-03", 0b0000, 0xFFFF_FFFF_0000_000F),
            ],
        );
        assert_eq!(groups, vec![vec!["left".to_string(), "again".to_string()]]);
    }

    #[test]
    fn test_similar_graph_joins_two_groups_when_a_bridge_arrives() {
        let mut graph = SimilarGraph::new(2);
        graph.add("x", "2026-01-01", 0b0000, 0);
        graph.add("y", "2026-01-02", 0b1111, 0);
        graph.scan();
        assert!(graph.partition().groups.is_empty());

        graph.add("bridge", "2026-01-03", 0b0011, 0);
        graph.scan();
        let partition = graph.partition();
        // 补一条边之后两枚孤张被串成一组，成员不重复
        assert_eq!(
            graph.member_ids(&partition),
            vec![vec!["x".to_string(), "y".to_string(), "bridge".to_string()]]
        );
    }

    #[test]
    fn test_similar_graph_routes_identical_hashes_through_one_representative() {
        let mut graph = SimilarGraph::new(3);
        for name in ["a", "b", "c"] {
            graph.add(name, "2026-01-01", 0b0101, 0b0011);
        }
        graph.add("far", "2026-01-02", 0xFFFF_FFFF_FFFF_FFF0, 0);
        graph.scan();
        let partition = graph.partition();
        // 同指纹只留一个代表进两两比对表，另外两张直接并进来
        assert_eq!(graph.reps.len(), 2);
        assert_eq!(
            graph.member_ids(&partition),
            vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]]
        );
    }

    #[test]
    fn test_similar_graph_orders_group_by_added_time() {
        let (groups, _) = clustered(
            3,
            &[
                ("late", "2026-03-01", 0b0000, 0),
                ("early", "2026-01-01", 0b0011, 0),
            ],
        );
        assert_eq!(groups, vec![vec!["early".to_string(), "late".to_string()]]);
    }

    /// 边是分两趟扫出来的，结果不能取决于这批图何时灌进来
    #[test]
    fn test_incremental_scan_matches_a_single_full_scan() {
        let items: [(&str, &str, u64, u64); 9] = [
            ("hub", "2026-01-01", 0, 0),
            ("n1", "2026-01-01", 0b0000_0011, 0),
            ("n1copy", "2026-01-04", 0b0000_0011, 0),
            ("n2", "2026-01-02", 0b0000_1100, 0),
            ("n3", "2026-01-03", 0b0011_0000, 0),
            ("n4", "2026-01-05", 0b1100_0000, 0),
            ("n5", "2026-01-06", 0b0000_0000_0000_0011_0000_0000_0000_0000, 0),
            ("other1", "2026-01-07", 0xFFFF_0000_0000_0003, 0xFFFF),
            ("other2", "2026-01-08", 0xFFFF_0000_0000_0001, 0xFFFF),
        ];
        let full = clustered(3, &items);

        let mut graph = SimilarGraph::new(3);
        for &(id, created, p, d) in &items[..5] {
            graph.add(id, created, p, d);
        }
        graph.scan();
        for &(id, created, p, d) in &items[5..] {
            graph.add(id, created, p, d);
        }
        graph.scan();
        let partition = graph.partition();
        assert_eq!((graph.member_ids(&partition), graph.far_ids(&partition)), full);
    }

    #[test]
    fn test_similar_signatures_cover_group_members_only() {
        let mut graph = SimilarGraph::new(3);
        graph.add("a", "2026-01-01", 0xFFFF_FFFF_0000_0001, 0);
        graph.add("b", "2026-01-02", 0xFFFF_FFFF_0000_0003, 0);
        graph.add("lonely", "2026-01-03", 0x0000_0000_0000_0000, 0);
        graph.scan();
        let partition = graph.partition();
        // 孤张也回指纹就是白传：19 万条库一次就是十几 MB
        let sigs = graph.signatures(&partition);
        assert_eq!(sigs.len(), 2);
        assert!(sigs.iter().all(|hit| hit.id != "lonely"));
        let a = sigs.iter().find(|hit| hit.id == "a").unwrap();
        // 高低 32 位拆开后要能拼回原值
        assert_eq!(((a.hi as u64) << 32) | a.lo as u64, 0xFFFF_FFFF_0000_0001);
    }

    #[test]
    fn test_deleted_image_frees_its_path() {
        let conn = setup_test_db();
        db::insert_image(&conn, &sample_image("i1", "C:/pics/a.png")).unwrap();
        assert!(db::is_image_path_taken(&conn, "C:/pics/a.png", "other").unwrap());

        db::delete_image(&conn, "i1").unwrap();
        assert!(!db::is_image_path_taken(&conn, "C:/pics/a.png", "other").unwrap());
    }

    #[test]
    fn test_undeleted_treats_missing_files_as_deleted() {
        // 文件早已被外部清掉：不该报错卡住整批，库记录要能跟着删
        let ghost = std::env::temp_dir().join("viewman-definitely-absent.jpg");
        let targets = vec![("i1".to_string(), ghost.to_string_lossy().to_string())];
        assert!(undeleted_targets(&targets).is_empty());
        assert!(undeleted_targets(&[]).is_empty());
    }

    #[test]
    fn test_checkpointed_images_are_skipped_on_the_next_scan() {
        let done = sample_image("i1", "D:/pics/a.png");
        // 已探测到尺寸的不再探，被打断的扫描因此能从下一张接着走
        assert!(!image_needs_probe(Some(&done), Some(done.file_size)));
        // 没入库的、尺寸缺失的、文件变了的都要探
        assert!(image_needs_probe(None, Some(done.file_size)));
        let mut no_dims = done.clone();
        no_dims.width = None;
        assert!(image_needs_probe(Some(&no_dims), Some(no_dims.file_size)));
        assert!(image_needs_probe(Some(&done), Some(done.file_size + 1)));
        // 读不到文件大小时宁可重探
        assert!(image_needs_probe(Some(&done), None));
    }

    #[test]
    fn test_checkpoint_persists_each_image_and_keeps_identity_on_rerun() {
        let conn = setup_test_db();
        let first = sample_image("i1", "D:/pics/a.png");
        db::insert_image(&conn, &first).unwrap();

        // 同一张图再次探测会带上新的 id/时间，落库必须沿用原记录，否则封面缓存会指向丢失的 id
        let mut again = first.clone();
        again.id = "fresh-id".into();
        again.created_at = "2026-09-23T00:00:00".into();
        again.thumbnail_path = Some("D:/thumb/a.jpg".to_string());
        db::insert_image(&conn, &again).unwrap();

        let rows = db::get_all_images(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "i1");
        assert_eq!(rows[0].thumbnail_path.as_deref(), Some("D:/thumb/a.jpg"));
    }
}
