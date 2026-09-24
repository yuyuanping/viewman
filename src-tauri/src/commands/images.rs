use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::models::Image;
use crate::scanner;

use super::scan::{compute_stale_ids, ScanProgress, ScanSummary};
use super::settings::{remember_root, IMAGE_SCAN_ROOTS_KEY};
use super::thumbnails::{cached_thumbnail_usable, clear_thumbnail_cache, run_thumbnail_jobs, ThumbJob};
use super::videos::{duplicate_signature, move_file};
use super::AppState;

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
) -> Result<Vec<Image>, String> {
    let existing_images = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_all_images(&conn).map_err(|e| e.to_string())?
    };

    let dir_path = PathBuf::from(&dir);
    let dir_for_task = dir.clone();
    let task_app = app.clone();
    let (images, stale_ids, warnings, summary) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<Image>, Vec<String>, Vec<String>, ScanSummary), String> {
            // 目录打不开时直接报错，绝不把"没扫到"当成"已删除"去清库
            let files = match scanner::scan_image_directory_recursive(&dir_path) {
                Ok(f) => f,
                Err(e) => return Err(format!("扫描中断，未修改数据库：{}", e)),
            };
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
            let prefix = format!("{}\\", dir_for_task.trim_end_matches('\\').to_lowercase());
            let stale_ids = compute_stale_ids(&existing, &prefix, &found);

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

            let (mut images, warnings) = build_images_parallel(&task_app, &new_files);
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

    Ok(images)
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

/// 挑出没能进回收站的那些：整批一次 shell 事务（`trash::delete_all` 内部只建一个
/// IFileOperation），失败再退回逐张，让被占用的那几张只影响自己。
/// 磁盘上已经不存在的一律视作已删除——库记录必须能清掉，否则残留显示。
fn filter_undeleted(targets: &[(String, String)]) -> Vec<(String, String)> {
    let present: Vec<&(String, String)> = targets
        .iter()
        .filter(|(_, path)| Path::new(path).exists())
        .collect();
    if present.is_empty() {
        return Vec::new();
    }

    let paths: Vec<&str> = present.iter().map(|(_, path)| path.as_str()).collect();
    if trash::delete_all(&paths).is_ok() {
        // 整批成功：仍在磁盘上的才算没删掉
        return present
            .into_iter()
            .filter(|(_, path)| Path::new(path).exists())
            .cloned()
            .collect();
    }

    // 一个文件被占用就足以让整批报错 → 退回逐张，只留下真正删不掉的那些
    let mut survivors = Vec::new();
    for (id, path) in present {
        if trash::delete(path).is_err() && Path::new(path).exists() {
            survivors.push((id.clone(), path.clone()));
        }
    }
    survivors
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
    let survivors = tauri::async_runtime::spawn_blocking(move || filter_undeleted(&for_files))
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

/// 找出内容相同的重复图片组：先按文件大小分组，再用内容指纹细分。
/// 每组按添加时间升序返回（第一个视为要保留的原件）
#[tauri::command]
pub async fn find_duplicate_images(state: State<'_, AppState>) -> Result<Vec<Vec<String>>, String> {
    let jobs: Vec<(String, String, i64, String)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_all_images(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|i| (i.id, i.path, i.file_size, i.created_at))
            .filter(|(_, p, _, _)| Path::new(p).exists())
            .collect()
    };

    let groups = tauri::async_runtime::spawn_blocking(move || {
        let mut by_size: HashMap<i64, Vec<(String, String, String)>> = HashMap::new();
        for (id, path, size, created_at) in jobs {
            by_size.entry(size).or_default().push((id, path, created_at));
        }
        let mut out: Vec<Vec<(String, String)>> = Vec::new();
        for (size, entries) in by_size {
            if entries.len() < 2 {
                continue;
            }
            let mut by_sig: HashMap<u64, Vec<(String, String)>> = HashMap::new();
            for (id, path, created_at) in entries {
                if let Some(sig) = duplicate_signature(&path, size) {
                    by_sig.entry(sig).or_default().push((id, created_at));
                }
            }
            for group in by_sig.into_values().filter(|g| g.len() > 1) {
                out.push(group);
            }
        }
        out.sort_by_key(|g| std::cmp::Reverse(g.len()));
        out.into_iter()
            .map(|mut g| {
                g.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                g.into_iter().map(|(id, _)| id).collect()
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(groups)
}

/// 找出"相似但不相同"的图片组（连拍/截图系列）：pHash 感知哈希 + 汉明距离 ≤10 聚类。
/// 与 find_duplicate_images 互补：字节级去重只认完全相同，这里抓视觉近似。
/// 个人库数量级下直接两两比对；组内按添加时间升序（第一个视为最佳保留候选）
#[tauri::command]
pub async fn find_similar_images(state: State<'_, AppState>) -> Result<Vec<Vec<String>>, String> {
    /// 汉明距离阈值：≤10 / 64 位视为相似（业界常用 8–12）
    const SIMILAR_THRESHOLD: u32 = 10;

    if !crate::scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法计算图片指纹。请安装 ffmpeg 并加入 PATH。".into());
    }

    let jobs: Vec<(String, String, String)> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_all_images(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|i| (i.id, i.path, i.created_at))
            .filter(|(_, p, _)| Path::new(p).exists())
            .collect()
    };

    let groups = tauri::async_runtime::spawn_blocking(move || {
        let mut hashed: Vec<(String, String, u64)> = Vec::new();
        for (id, path, _) in &jobs {
            if let Some(h) = crate::scanner::image_phash(path) {
                hashed.push((id.clone(), path.clone(), h));
            }
        }
        // 并查集聚类：距离 ≤ 阈值即并成一组
        let mut parent: Vec<usize> = (0..hashed.len()).collect();
        fn find(parent: &mut Vec<usize>, i: usize) -> usize {
            if parent[i] != i {
                let root = find(parent, i);
                parent[i] = root;
            }
            parent[i]
        }
        for i in 0..hashed.len() {
            for j in (i + 1)..hashed.len() {
                if crate::scanner::hamming_distance(hashed[i].2, hashed[j].2) <= SIMILAR_THRESHOLD {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    if ri != rj {
                        parent[ri] = rj;
                    }
                }
            }
        }
        // 收组；加入时间映射回原表排序
        let created: HashMap<String, String> = jobs.iter().map(|(id, _, created)| (id.clone(), created.clone())).collect();
        let mut by_root: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, item) in hashed.iter().enumerate() {
            by_root.entry(find(&mut parent, i)).or_default().push(item.0.clone());
        }
        let mut out: Vec<Vec<String>> = by_root
            .into_values()
            .filter(|g| g.len() > 1)
            .map(|mut g| {
                g.sort_by(|a, b| {
                    created.get(a).unwrap_or(&String::new()).cmp(created.get(b).unwrap_or(&String::new()))
                        .then_with(|| a.cmp(b))
                });
                g
            })
            .collect();
        out.sort_by_key(|g| std::cmp::Reverse(g.len()));
        out
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(filter_undeleted(&targets).is_empty());
        assert!(filter_undeleted(&[]).is_empty());
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
