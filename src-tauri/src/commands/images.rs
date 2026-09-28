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
use super::thumbnails::{cached_thumbnail_usable, clear_thumbnail_cache, run_thumbnail_jobs, ThumbEntry, ThumbJob};
use super::videos::{move_file, same_target_dir};
use super::{undeleted_targets, AppState, MapErrStr};

/// 扫描任务的完整产出：新文件、可清理的旧 id、告警、摘要。
type ImageScanOutcome = Result<(Vec<Image>, Vec<String>, Vec<String>, ScanSummary), String>;

#[tauri::command]
pub fn get_images(state: State<AppState>) -> Result<Vec<Image>, String> {
    let conn = state.db.lock().map_err_str()?;
    db::get_all_images(&conn).map_err_str()
}

/// 图片库视图：扫描范围 + 目录前缀 + 文件名子串过滤下推到 SQL，前端不再整表过桥。
/// 19 万行的查询 + JSON 序列化是秒级开销，扔进阻塞线程池，别堵主线程（同步命令跑在主线程上）。
#[tauri::command]
pub async fn get_image_view(
    app: tauri::AppHandle,
    dir: Option<String>,
    search: String,
) -> Result<db::ImageView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        let roots = super::settings::load_roots(&conn, IMAGE_SCAN_ROOTS_KEY);
        db::get_image_view(&conn, dir.as_deref(), &search, &roots).map_err_str()
    })
    .await
    .map_err_str()?
}

/// 图片库统计：总数、缺封面数、每父目录直接文件数（目录树与每根计数的数据源）。
/// 只统计扫描范围内的文件。
#[tauri::command]
pub async fn get_image_stats(app: tauri::AppHandle) -> Result<db::ImageStats, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        let roots = super::settings::load_roots(&conn, IMAGE_SCAN_ROOTS_KEY);
        db::get_image_stats(&conn, &roots).map_err_str()
    })
    .await
    .map_err_str()?
}

/// 缺封面图片的 id 集：封面批任务的待办清单
#[tauri::command]
pub async fn get_missing_image_thumbnail_ids(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        db::get_missing_image_thumbnail_ids(&conn).map_err_str()
    })
    .await
    .map_err_str()?
}

/// 按 id 批量取图：检测面板元数据与"id 还活着吗"的收敛判定。
/// 按扫描根过滤口径：移出根外的落点虽然记录还在，面板也当它已退库
#[tauri::command]
pub async fn get_images_by_ids(app: tauri::AppHandle, ids: Vec<String>) -> Result<Vec<Image>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        let roots = super::settings::load_roots(&conn, IMAGE_SCAN_ROOTS_KEY);
        db::get_images_by_ids(&conn, &ids, &roots).map_err_str()
    })
    .await
    .map_err_str()?
}

/// 递归扫描图片目录并增量更新图片库：新文件探测尺寸，扫描时已消失的文件从库里清掉。
/// 与视频扫描各走各的表，同一个目录可以被两边分别收录。
#[tauri::command]
pub async fn scan_image_directory(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<ScanOutcome<Image>, String> {
    // 只拉本根前缀下的旧账：失效清理与新旧比对都只关心这个根，
    // 全表 19 万行拉一遍会把视图/统计/其它扫描堵在数据库锁后面好几秒
    let existing_images = {
        let conn = state.db.lock().map_err_str()?;
        db::get_images_under_prefix(&conn, &dir_prefix_lower(&dir)).map_err_str()?
    };

    let dir_path = PathBuf::from(&dir);
    let dir_for_task = dir.clone();
    let task_app = app.clone();
    let (images, stale_ids, warnings, summary) =
        tauri::async_runtime::spawn_blocking(move || -> ImageScanOutcome {
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
        .map_err_str()??;

    {
        let conn = state.db.lock().map_err_str()?;
        // 新条目已在探测时逐张落库，这里只清外部已删除的失效条目
        if !stale_ids.is_empty() {
            let tx = conn.unchecked_transaction().map_err_str()?;
            db::delete_images_by_ids(&tx, &stale_ids).map_err_str()?;
            tx.commit().map_err_str()?;
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

/// 删除目标：[(id, 磁盘路径)] 与"库里已查无记录、视作已删除"的 id 清单
type DeleteTargets = Result<(Vec<(String, String)>, Vec<String>), String>;

/// 按 id 查删除目标：库里还挂着记录的归"去磁盘上删"，查无记录的归"视作已删除"
/// （重扫换代后，恢复出来的重复分组缓存里挂的还是上一代的 id）。
/// 查无记录虽无从删起，但一票否决会让整批都删不动。
fn split_known_targets(conn: &rusqlite::Connection, image_ids: &[String]) -> DeleteTargets {
    let mut targets = Vec::with_capacity(image_ids.len());
    let mut gone = Vec::new();
    for image_id in image_ids {
        match db::get_image_path(conn, image_id).map_err_str()? {
            Some(path) => targets.push((image_id.clone(), path)),
            None => gone.push(image_id.clone()),
        }
    }
    Ok((targets, gone))
}

/// 扩展名修正的统计
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionFixReport {
    pub renamed: usize,
    pub already_matched: usize,
    /// 认不出内容格式的（真坏文件/非图片数据），原样保留
    pub unrecognized: usize,
    /// 想改但没改成（文件被占用等）
    pub failed: usize,
}

/// 按文件头认内容格式。认不出的返回 None——改名只对认得出的做，宁可留着也别猜。
/// 现有库名单里错标的就这几种：PNG 当 .jpg/.bmp 存、JPEG 当 .png/.bmp 存、
/// WebP/GIF 当 .jpg 存（顺手把 avif 也认了，虽然库里还没见过）。
fn content_extension(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 12];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if head.starts_with(b"\xff\xd8\xff") {
        return Some("jpg");
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if head.starts_with(b"RIFF") && head[8..12] == *b"WEBP" {
        return Some("webp");
    }
    if head[4..8] == *b"ftyp" && (&head[8..12] == b"avif" || &head[8..12] == b"avis") {
        return Some("avif");
    }
    // "BM" 只有两字节，靠文件长度字段（第 3~6 字节 LE）不超过实际大小挡一挡文本撞车
    if head.starts_with(b"BM") {
        let declared = u32::from_le_bytes([head[2], head[3], head[4], head[5]]);
        let actual = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if declared == 0 || (declared as u64) <= actual {
            return Some("bmp");
        }
    }
    None
}

/// 扩展名（小写）与内容格式对不对得上；jpg/jpeg 都算 JPEG 内容的相符写法
fn extension_matches(current: &str, content: &str) -> bool {
    match content {
        "jpg" => current == "jpg" || current == "jpeg",
        other => current == other,
    }
}

/// 批量修正扩展名与内容不符的图片：只看还没有指纹的在库文件（重复检测里
/// "解不出画面"的那批——解得出的图 ffmpeg 按内容探测兜着，错标不碍比对）。
/// 按文件头认格式就地改名，库记录路径同步改掉，不必重扫；认不出的原样保留。
#[tauri::command]
pub async fn fix_mismatched_extensions(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ExtensionFixReport, String> {
    let rows: Vec<(String, String)> = {
        let conn = state.db.lock().map_err_str()?;
        db::unhashed_image_paths(&conn).map_err_str()?
    };
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mut report = ExtensionFixReport { renamed: 0, already_matched: 0, unrecognized: 0, failed: 0 };
        // 文件探测与改名全程不持数据库锁：锁只圈住最后的路径写库
        let mut renamed: Vec<(String, PathBuf, PathBuf)> = Vec::new(); // (id, 新路径, 原路径)
        for (id, path) in rows {
            let p = Path::new(&path);
            // 磁盘上已经不在的记录等扫描去清，这里不管
            if !p.exists() {
                continue;
            }
            let Some(ext) = content_extension(p) else {
                report.unrecognized += 1;
                continue;
            };
            let current = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default();
            if extension_matches(&current, ext) {
                report.already_matched += 1;
                continue;
            }
            // 目标重名时加 " (2)" 后缀，口径同 move_image
            let (Some(dir), Some(stem)) = (p.parent(), p.file_stem()) else {
                report.failed += 1;
                continue;
            };
            let stem = stem.to_string_lossy().to_string();
            let mut target = dir.join(format!("{stem}.{ext}"));
            let mut n = 2;
            while target.exists() {
                target = dir.join(format!("{stem} ({n}).{ext}"));
                n += 1;
            }
            if std::fs::rename(p, &target).is_err() {
                report.failed += 1;
                continue;
            }
            renamed.push((id, target, p.to_path_buf()));
        }
        // 写库失败的把文件移回原位：库记录和磁盘必须说同一个故事（回退改名在锁外做）
        let mut rollback: Vec<PathBuf> = Vec::new(); // 写库失败的新路径
        {
            let conn = state.db.lock().map_err_str()?;
            for (id, target, _) in &renamed {
                if db::update_image_path(&conn, id, &target.to_string_lossy()).is_ok() {
                    report.renamed += 1;
                } else {
                    rollback.push(target.clone());
                }
            }
        }
        for (_, target, original) in &renamed {
            if rollback.contains(target) {
                let _ = std::fs::rename(target, original);
                report.failed += 1;
            }
        }
        Ok(report)
    })
    .await
    .map_err_str()?
}

/// 批量删除图片：一次回收站事务 + 一次数据库事务，返回成功删除的 id。
/// 逐张删时每张都要过一次 IPC、一次 shell 调用和一次事务落盘，勾选几百张就是十几秒。
/// 库里已经没有记录的 id 视作已删除，见 split_known_targets。
#[tauri::command]
pub async fn delete_images(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    image_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    let (targets, mut deleted): (Vec<(String, String)>, Vec<String>) = {
        let conn = state.db.lock().map_err_str()?;
        split_known_targets(&conn, &image_ids)?
    };

    let for_files = targets.clone();
    let survivors = tauri::async_runtime::spawn_blocking(move || undeleted_targets(&for_files))
        .await
        .map_err_str()?;
    let failed: HashSet<String> = survivors.into_iter().map(|(id, _)| id).collect();
    deleted.extend(
        targets
            .iter()
            .filter(|(id, _)| !failed.contains(id))
            .map(|(id, _)| id.clone()),
    );

    if !deleted.is_empty() {
        let conn = state.db.lock().map_err_str()?;
        let tx = conn.unchecked_transaction().map_err_str()?;
        db::delete_images_by_ids(&tx, &deleted).map_err_str()?;
        tx.commit().map_err_str()?;
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
        let conn = state.db.lock().map_err_str()?;
        let old_path = db::get_image_path(&conn, &image_id)
            .map_err_str()?
            .ok_or_else(|| format!("Image not found: {}", image_id))?;
        let src = Path::new(&old_path);
        let filename = src
            .file_name()
            .ok_or_else(|| "源文件路径无效".to_string())?
            .to_string_lossy()
            .to_string();
        let dir = Path::new(&target_dir);
        if same_target_dir(&old_path, &target_dir) {
            return Err("文件已经在该目录中".into());
        }
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
        let ext = src.extension().and_then(|s| s.to_str());
        let mut candidate = dir.join(&filename);
        let mut idx = 1;
        while candidate.exists()
            || db::is_image_path_taken(&conn, &candidate.to_string_lossy(), &image_id)
                .map_err_str()?
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
    .map_err_str()?;
    move_result?;

    if let Err(e) = state
        .db
        .lock()
        .map_err_str()
        .and_then(|conn| db::update_image_path(&conn, &image_id, &new_path).map_err_str())
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
    // 最小行集在锁内一次拉完；"缓存文件还在吗"的逐条 stat 挪到锁外，
    // 免得几万次 metadata 让其它命令排队等数据库锁
    let candidates: Vec<ThumbEntry> = {
        let conn = state.db.lock().map_err_str()?;
        db::get_image_thumbnail_entries(&conn)
            .map_err_str()?
            .into_iter()
            .map(|(id, source, thumbnail_path)| ThumbEntry { id, source, duration: None, thumbnail_path })
            .collect()
    };
    let jobs: Vec<ThumbJob> = candidates
        .into_iter()
        .filter(|e| image_ids.iter().any(|id| id == &e.id))
        .filter(|e| !cached_thumbnail_usable(&e.thumbnail_path))
        .map(|e| ThumbJob { id: e.id, source: e.source, duration: None })
        .collect();

    run_thumbnail_jobs(&app, jobs, "image-thumbnail-progress", db::set_image_thumbnail).await
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use crate::db::{sample_image, setup_test_db};

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
    fn test_unknown_ids_are_sorted_out_instead_of_failing_the_batch() {
        // 重扫换代后缓存里挂的上一代 id 查无记录：分离出来当已删除，不能卡住整批
        let conn = setup_test_db();
        db::insert_image(&conn, &sample_image("i1", "C:/pics/a.png")).unwrap();
        let ids = ["i1".to_string(), "stale-id".to_string()];
        let (targets, gone) = split_known_targets(&conn, &ids).unwrap();
        assert_eq!(targets, vec![("i1".to_string(), "C:/pics/a.png".to_string())]);
        assert_eq!(gone, vec!["stale-id".to_string()]);
    }

    #[test]
    fn test_content_extension_reads_magic_bytes() {
        let dir = std::env::temp_dir().join(format!("viewman_extfix_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            fs::write(&p, bytes).unwrap();
            p
        };
        let png = write("x.png", b"\x89PNG\r\n\x1a\n whatever");
        assert_eq!(content_extension(&png), Some("png"));
        let jpg = write("x.jpg", b"\xff\xd8\xff\xe0 jfif-ish");
        assert_eq!(content_extension(&jpg), Some("jpg"));
        let gif = write("x.gif", b"GIF89a drawing");
        assert_eq!(content_extension(&gif), Some("gif"));
        // BMP 长度字段（LE）在声明范围内才算，纯文本撞上 "BM" 两个字母的不认
        let mut bmp_head = Vec::from(&b"BM"[..]);
        bmp_head.extend_from_slice(&70u32.to_le_bytes());
        bmp_head.extend_from_slice(&[0u8; 64]);
        let bmp = write("x.bmp", &bmp_head);
        assert_eq!(content_extension(&bmp), Some("bmp"));
        let fake_bmp = write("y.bmp", b"BM this is just text talking");
        assert_eq!(content_extension(&fake_bmp), None);
        let junk = write("x.jpg", b"{\"retcode\":100055}");
        assert_eq!(content_extension(&junk), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_extension_matches_tolerates_jpeg_spelling() {
        assert!(extension_matches("jpg", "jpg"));
        assert!(extension_matches("jpeg", "jpg"));
        assert!(extension_matches("png", "png"));
        assert!(!extension_matches("jpg", "png"));
        assert!(!extension_matches("bmp", "png"));
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
