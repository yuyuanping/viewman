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
use super::videos::{duplicate_signature, move_file};
use super::{undeleted_targets, AppState};

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

/// 相似图检测的进度事件负载（事件名 similar-progress）
#[derive(Clone, serde::Serialize)]
pub struct SimilarProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    /// 这一批里被碰过的组，给的是合并后的完整成员（前端按重叠并入已有结果）
    pub groups: Vec<Vec<String>>,
}

/// 一张成组图片的指纹：u64 拆成两个 u32，前端按 JS number 做异或数 1，不必碰 BigInt
#[derive(Clone, serde::Serialize)]
pub struct SimilarHit {
    pub id: String,
    pub lo: u32,
    pub hi: u32,
}

/// 检测结论：分组 + 组内各成员的指纹。带上指纹，面板才能按"距保留张多远"给胖组排序
#[derive(Clone, serde::Serialize)]
pub struct SimilarResult {
    pub groups: Vec<Vec<String>>,
    pub hashes: Vec<SimilarHit>,
}

/// pHash 增量聚类：每灌进一张新图只和已收进来的图比一次，整趟代价仍是一次两两比对；
/// 换来的是"每攒够一批就能吐出已成形的组"，前端不必等全库算完才开始审。
/// 组件之间出现桥接时把小的并进大的（同时更新成员的归属），所以合并是平摊 O(n log n)。
struct SimilarCluster<'a> {
    threshold: u32,
    hashes: Vec<u64>,
    ids: Vec<&'a str>,
    created: Vec<&'a str>,
    /// 节点 → 组件槽位；组件被并掉后槽位留空，不复用，免得旧索引指到别组
    comp_of: Vec<usize>,
    comps: Vec<Vec<usize>>,
    /// 指纹互不相同的代表节点：两两比对只走这张表
    reps: Vec<usize>,
    rep_of: HashMap<u64, usize>,
}

impl<'a> SimilarCluster<'a> {
    fn new(threshold: u32) -> Self {
        SimilarCluster {
            threshold,
            hashes: Vec::new(),
            ids: Vec::new(),
            created: Vec::new(),
            comp_of: Vec::new(),
            comps: Vec::new(),
            reps: Vec::new(),
            rep_of: HashMap::new(),
        }
    }

    /// 收进一张图的哈希，返回它所在组件的槽位
    fn add(&mut self, id: &'a str, created: &'a str, hash: u64) -> usize {
        let node = self.hashes.len();
        self.hashes.push(hash);
        self.ids.push(id);
        self.created.push(created);

        // 指纹完全相同 ⇒ 距离 0，直接进那张图的组件，一次比对都不用
        if let Some(&rep) = self.rep_of.get(&hash) {
            let home = self.comp_of[rep];
            self.comps[home].push(node);
            self.comp_of.push(home);
            return home;
        }
        self.rep_of.insert(hash, node);

        let mut targets: Vec<usize> = Vec::new();
        for &other in &self.reps {
            if scanner::hamming_distance(self.hashes[other], hash) <= self.threshold {
                let comp = self.comp_of[other];
                if !targets.contains(&comp) {
                    targets.push(comp);
                }
            }
        }
        self.reps.push(node);

        let home = if targets.is_empty() {
            self.comps.push(vec![node]);
            self.comps.len() - 1
        } else {
            targets.sort_by_key(|&comp| std::cmp::Reverse(self.comps[comp].len()));
            let home = targets[0];
            for index in 1..targets.len() {
                for member in std::mem::take(&mut self.comps[targets[index]]) {
                    self.comp_of[member] = home;
                    self.comps[home].push(member);
                }
            }
            self.comps[home].push(node);
            home
        };
        self.comp_of.push(home);
        home
    }

    fn len_of(&self, comp: usize) -> usize {
        self.comps[comp].len()
    }

    /// 组内按加入时间升序（首张即最早入库的原件），时间相同按 id
    fn member_ids(&self, comp: usize) -> Vec<String> {
        let mut members = self.comps[comp].clone();
        members.sort_by_key(|&node| (self.created[node], self.ids[node]));
        members.into_iter().map(|node| self.ids[node].to_string()).collect()
    }

    /// 全量结果：组员 ≥2 的组，大的排前面
    fn groups(&self) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = (0..self.comps.len())
            .filter(|&comp| self.comps[comp].len() > 1)
            .map(|comp| self.member_ids(comp))
            .collect();
        out.sort_by_key(|group| std::cmp::Reverse(group.len()));
        out
    }

    /// 成组图片的指纹（孤张不给，免得 19 万条清单白传一趟）
    fn signatures(&self) -> Vec<SimilarHit> {
        let mut out = Vec::new();
        for comp in 0..self.comps.len() {
            if self.comps[comp].len() < 2 {
                continue;
            }
            for &node in &self.comps[comp] {
                let hash = self.hashes[node];
                out.push(SimilarHit {
                    id: self.ids[node].to_string(),
                    lo: hash as u32,
                    hi: (hash >> 32) as u32,
                });
            }
        }
        out
    }
}

/// 把"这一批碰到过的组"推给前端；done 时不带成员（完整结果走命令返回值）
fn emit_similar_progress(
    app: &tauri::AppHandle,
    processed: usize,
    total: usize,
    done: bool,
    cluster: &SimilarCluster,
    touched: &mut HashSet<usize>,
) {
    let groups: Vec<Vec<String>> = if done {
        Vec::new()
    } else {
        touched.iter()
            .filter(|&&comp| cluster.len_of(comp) > 1)
            .map(|&comp| cluster.member_ids(comp))
            .collect()
    };
    touched.clear();
    let _ = app.emit(
        "similar-progress",
        SimilarProgress { processed, total, done, groups },
    );
}

/// 找出"相似但不相同"的图片组（连拍/截图系列）：pHash 感知哈希 + 汉明距离 ≤ threshold 聚类。
/// 与 find_duplicate_images 互补：字节级去重只认完全相同，这里抓视觉近似。
/// 指纹算过一次就落在 images.phash，只有新图/改过的图才再跑 ffmpeg；已缓存的部分先聚好推出去，
/// 剩下的边算边经 similar-progress 吐组，命令返回值是最终完整结果 + 组内各成员的指纹
/// （指纹给前端算"距保留张 N 位"：胖组靠它把最像的排前面、把只隔几级的远亲折起来）。
#[tauri::command]
pub async fn find_similar_images(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    threshold: u32,
) -> Result<SimilarResult, String> {
    /// 每补算这么多张推一次进度、并把攒好的指纹落库
    const EMIT_EVERY: usize = 256;
    let threshold = threshold.clamp(1, 32);

    if !crate::scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法计算图片指纹。请安装 ffmpeg 并加入 PATH。".into());
    }

    // 文件已经不在磁盘上的条目不参与聚类，也不用再白跑一遍解码
    let rows: Vec<db::PhashRow> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_phash_rows(&conn).map_err(|e| e.to_string())?
    }
    .into_iter()
    .filter(|row| Path::new(&row.path).exists())
    .collect();

    let result = tauri::async_runtime::spawn_blocking(move || -> SimilarResult {
        let mut cluster = SimilarCluster::new(threshold);
        let mut touched: HashSet<usize> = HashSet::new();
        let mut pending: Vec<&db::PhashRow> = Vec::new();
        for row in &rows {
            match row.cached_hash() {
                Some(hash) => {
                    touched.insert(cluster.add(row.id.as_str(), row.created_at.as_str(), hash));
                }
                None => pending.push(row),
            }
        }
        let total = pending.len();
        // 缓存里已有的那部分不花钱：先把已经能看的组推出去，面板不用等补算完
        emit_similar_progress(&app, 0, total, false, &cluster, &mut touched);

        let mut processed = 0usize;
        let mut fresh: Vec<(String, i64, Option<String>)> = Vec::with_capacity(EMIT_EVERY);
        for row in pending {
            if let Some(hash) = crate::scanner::image_phash(&row.path) {
                touched.insert(cluster.add(row.id.as_str(), row.created_at.as_str(), hash));
                fresh.push((row.id.clone(), hash as i64, row.modified_at.clone()));
            }
            processed += 1;
            if processed % EMIT_EVERY == 0 || processed == total {
                if let Ok(conn) = app.state::<AppState>().db.lock() {
                    let _ = db::save_image_phashes(&conn, &fresh);
                }
                fresh.clear();
                emit_similar_progress(&app, processed, total, false, &cluster, &mut touched);
            }
        }
        if let Ok(conn) = app.state::<AppState>().db.lock() {
            let _ = db::save_image_phashes(&conn, &fresh);
        }

        emit_similar_progress(&app, processed, total, true, &cluster, &mut touched);
        SimilarResult { groups: cluster.groups(), hashes: cluster.signatures() }
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{sample_image, setup_test_db};

    #[test]
    fn test_similar_cluster_chains_transitively_and_isolates_outliers() {
        let mut cluster = SimilarCluster::new(3);
        for (name, hash) in [
            ("a", 0b0000u64),
            ("b", 0b0011),
            ("c", 0b1111),
            ("d", 0xFFFF_FFFF_FFFF_FFF0),
        ] {
            cluster.add(name, "2026-01-01", hash);
        }
        // a~b、b~c 成立，a 与 c 距离 4 已超阈值：仍应靠 b 串成一组，d 单独不算组
        assert_eq!(cluster.groups(), vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]]);
    }

    #[test]
    fn test_similar_cluster_joins_two_groups_when_a_bridge_arrives() {
        let mut cluster = SimilarCluster::new(2);
        cluster.add("x", "2026-01-01", 0b0000);
        cluster.add("y", "2026-01-02", 0b1111);
        assert_eq!(cluster.groups(), Vec::<Vec<String>>::new());

        cluster.add("bridge", "2026-01-03", 0b0011);
        assert_eq!(
            cluster.groups(),
            vec![vec!["x".to_string(), "y".to_string(), "bridge".to_string()]]
        );
        // 并组后旧槽位清空：成员总数不变，全量结果里也不会重复出现
        let members: usize = (0..cluster.comps.len()).map(|comp| cluster.len_of(comp)).sum();
        assert_eq!(members, 3);
    }

    #[test]
    fn test_similar_cluster_routes_identical_hashes_through_one_representative() {
        let mut cluster = SimilarCluster::new(3);
        for name in ["a", "b", "c"] {
            cluster.add(name, "2026-01-01", 0b0101);
        }
        cluster.add("far", "2026-01-02", 0xFFFF_FFFF_FFFF_FFF0);
        // 同指纹只留一个代表进两两比对表，另外两张直接并进来
        assert_eq!(cluster.reps.len(), 2);
        assert_eq!(
            cluster.groups(),
            vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]]
        );
    }

    #[test]
    fn test_similar_cluster_orders_group_by_added_time() {
        let mut cluster = SimilarCluster::new(3);
        cluster.add("late", "2026-03-01", 0b0000);
        cluster.add("early", "2026-01-01", 0b0011);
        assert_eq!(cluster.groups(), vec![vec!["early".to_string(), "late".to_string()]]);
    }

    #[test]
    fn test_similar_signatures_cover_group_members_only() {
        let mut cluster = SimilarCluster::new(3);
        cluster.add("a", "2026-01-01", 0xFFFF_FFFF_0000_0001);
        cluster.add("b", "2026-01-02", 0xFFFF_FFFF_0000_0003);
        cluster.add("lonely", "2026-01-03", 0x0000_0000_0000_0000);
        // 孤张也回指纹就是白传：19 万条库一次就是十几 MB
        let sigs = cluster.signatures();
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
