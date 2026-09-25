use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::models::{ScanOutcome, Video};
use crate::scanner;

use super::settings::{remember_root, SCAN_ROOTS_KEY};
use super::{AppState, MapErrStr};

#[derive(Clone, serde::Serialize)]
pub struct ScanProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    pub warnings: Vec<String>,
    /// 扫描摘要（done=true 时有效）：本次新增 / 移除 / 元数据刷新的条目数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ScanSummary>,
}

/// 一次扫描的变更统计，供前端在完成 Toast 里展示
#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct ScanSummary {
    pub added: usize,
    pub removed: usize,
    pub refreshed: usize,
}

#[tauri::command]
pub async fn scan_directory(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<ScanOutcome<Video>, String> {
    let existing_videos = {
        let conn = state.db.lock().map_err_str()?;
        db::get_all_videos(&conn).map_err_str()?
    };

    let dir_path = std::path::PathBuf::from(&dir);
    let dir_for_task = dir.clone();
    let task_app = app.clone();
    let (videos, stale_ids, warnings, summary) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<Video>, Vec<String>, Vec<String>, ScanSummary), String> {
            // 根目录打不开时直接报错，绝不把"没扫到"当成"已删除"去清库
            let walk = match scanner::scan_directory_recursive(&dir_path) {
                Ok(w) => w,
                Err(e) => return Err(format!("扫描中断，未修改数据库：{}", e)),
            };
            let files = walk.files;
            let mut walk_warnings = capped_walk_warnings(walk.warnings);
            // 读得到目录就先记下根：本轮扫描即使被打断，下次启动也会接着扫
            if let Ok(conn) = task_app.state::<AppState>().db.lock() {
                let _ = remember_root(&conn, SCAN_ROOTS_KEY, &dir_for_task);
            }

            let found: HashSet<String> = files
                .iter()
                .map(|f| f.to_string_lossy().to_lowercase())
                .collect();
            let existing: Vec<_> = existing_videos.iter()
                .map(|v| (v.id.clone(), v.path.clone())).collect();
            let by_path: std::collections::HashMap<_, _> = existing_videos.iter()
                .map(|v| (v.path.to_lowercase(), v)).collect();

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
                    match by_path.get(&f.to_string_lossy().to_lowercase()) {
                        None => true,
                        Some(old) => old.duration.is_none() || old.width.is_none() || old.height.is_none()
                            || std::fs::metadata(f).map(|m| m.len() as i64 != old.file_size).unwrap_or(true),
                    }
                })
                .collect();

            let _ = task_app.emit(
                "scan-progress",
                ScanProgress { processed: 0, total: new_files.len(), done: false, warnings: vec![], summary: None },
            );

            let (mut videos, probe_warnings) = build_videos_parallel(&task_app, &new_files);
            let mut warnings = walk_warnings;
            warnings.extend(probe_warnings);
            let mut refreshed = 0;
            for video in &mut videos {
                if let Some(old) = by_path.get(&video.path.to_lowercase()) {
                    refreshed += 1;
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
            // added = 探测成功的条目里没在旧库出现过的；refreshed = 命中旧库被刷新的
            let added = videos.iter().filter(|v| !by_path.contains_key(&v.path.to_lowercase())).count();
            let summary = Ok((videos, stale_ids, warnings, ScanSummary { added, removed: 0, refreshed }));
            summary
        })
        .await
        .map_err_str()??;

    {
        let conn = state.db.lock().map_err_str()?;
        // 新条目已在探测时逐条落库，这里只清外部已删除的失效条目
        if !stale_ids.is_empty() {
            let tx = conn.unchecked_transaction().map_err_str()?;
            db::delete_videos_by_ids(&tx, &stale_ids).map_err_str()?;
            tx.commit().map_err_str()?;
        }
    }

    let _ = app.emit(
        "scan-progress",
        ScanProgress { processed: videos.len(), total: videos.len(), done: true, warnings: warnings.clone(), summary: Some(ScanSummary { added: summary.added, removed: stale_ids.len(), refreshed: summary.refreshed }) },
    );

    Ok(ScanOutcome { items: videos, removed_ids: stale_ids })
}

/// 目录 → 大小写不敏感的前缀比较键（统一带结尾分隔符，避免 "D:\v" 误匹配 "D:\vids2"）。
pub(crate) fn dir_prefix_lower(dir: &str) -> String {
    format!("{}\\", dir.trim_end_matches(['\\', '/']).to_lowercase())
}

/// 走查告警会进 Toast，全盘扫描一次可能产出上百条：点名前几条，其余并成一句统计。
pub(crate) fn capped_walk_warnings(warnings: Vec<String>) -> Vec<String> {
    const LIMIT: usize = 3;
    if warnings.len() <= LIMIT {
        return warnings;
    }
    let extra = warnings.len() - LIMIT;
    let mut out = warnings.into_iter().take(LIMIT).collect::<Vec<_>>();
    out.push(format!("另有 {} 个目录未能读取", extra));
    out
}

/// 决定本轮可以清掉哪些"没扫到"的库记录，两道保险：
/// 1. 整片没读到的子树（权限不足、网络盘掉线、有意不跟随的链接目录）里的记录不算失效；
///    这类目录一多，逐条比前缀不划算，直接放弃本轮清理；
/// 2. 要清的数量远超本轮扫到的文件数，更像目录没读全而不是用户真删了——放弃清理并告警。
/// 清库会连带缩略图缓存一起没了且不可回退，拿不准时宁可留着等下一轮。
pub(crate) fn plan_stale_ids(
    existing: &[(String, String)],
    prefix: &str,
    found_lower: &HashSet<String>,
    skipped: &[std::path::PathBuf],
    warnings: &mut Vec<String>,
) -> Vec<String> {
    const MAX_TRACKED: usize = 20;
    if skipped.len() > MAX_TRACKED {
        warnings.push(format!("有 {} 个目录本轮未能读取，已跳过失效清理", skipped.len()));
        return Vec::new();
    }
    let skip_prefixes: Vec<String> = skipped
        .iter()
        .map(|dir| dir_prefix_lower(&dir.to_string_lossy()))
        .collect();
    let stale = compute_stale_ids(existing, prefix, found_lower, &skip_prefixes);
    vet_mass_prune(stale, found_lower.len(), warnings)
}

/// 返回 should-delete 的 id：路径在 prefix 目录下（大小写不敏感）、不在 found 里，
/// 也不在 skipped_prefixes 指向的任何子树里。
pub(crate) fn compute_stale_ids(
    existing: &[(String, String)],
    prefix: &str,
    found_lower: &HashSet<String>,
    skipped_prefixes: &[String],
) -> Vec<String> {
    existing
        .iter()
        .filter(|(_, path)| {
            let lp = path.to_lowercase();
            lp.starts_with(prefix)
                && !found_lower.contains(&lp)
                && !skipped_prefixes.iter().any(|skip| lp.starts_with(skip.as_str()))
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// 大批记录被判定为"已删除"、但本轮目录下根本没找到几个文件：更可能是网络盘半掉线、
/// 移动硬盘睡了、根目录被换成了空的。正常批量删除不会被拦——那时留下的文件数和
/// 删掉的数量同量级。
fn vet_mass_prune(
    mut stale_ids: Vec<String>,
    found_count: usize,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    const MIN_BATCH: usize = 100;
    if stale_ids.len() > MIN_BATCH && stale_ids.len() > found_count {
        warnings.push(format!(
            "{} 条记录本次未扫到，但目录下只找到 {} 个文件，疑似目录未被完整读取，已跳过清理（确认无误可在侧栏\"移除目录\"手动处理）",
            stale_ids.len(),
            found_count
        ));
        stale_ids.clear();
    }
    stale_ids
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
    let write_failures = AtomicUsize::new(0);
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
                        // 探一条落一条：中途退出应用，下次扫描从这里接着走而不是从头再来
                        match app.state::<AppState>().db.lock() {
                            Ok(conn) => {
                                if db::insert_video(&conn, &video).is_err() {
                                    write_failures.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(_) => {
                                write_failures.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        out.push(video);
                        let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = app.emit(
                            "scan-progress",
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
            "{} 个视频未能获取时长（请确认已安装 ffmpeg/ffprobe，或文件本身损坏）",
            failures
        ));
    }
    let missed = write_failures.load(Ordering::Relaxed);
    if missed > 0 {
        warnings.push(format!("{} 个视频记录写入数据库失败，下次扫描会重试", missed));
    }
    (videos, warnings)
}

#[cfg(test)]
mod tests {
    use super::{capped_walk_warnings, compute_stale_ids, dir_prefix_lower, plan_stale_ids};

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

        let stale = compute_stale_ids(&existing, r"d:\vid\", &found, &[]);
        assert_eq!(stale, vec!["b".to_string()]);
    }

    #[test]
    fn test_compute_stale_ids_prefix_boundary() {
        // "D:\v" 不能误匹配 "D:\vids2\x.mp4"
        let existing = vec![("c".to_string(), r"D:\vids2\three.mp4".to_string())];
        let found: std::collections::HashSet<String> = std::collections::HashSet::new();
        let stale = compute_stale_ids(&existing, r"d:\v\", &found, &[]);
        assert!(stale.is_empty());
    }

    /// 整片没读到的子树（权限不足、网络盘掉线、不跟随的链接目录）里的记录必须留着
    /// 等下一轮，否则一次不完整的扫描就会把好端端的库清空
    #[test]
    fn test_compute_stale_ids_spares_skipped_subtree() {
        let existing = vec![
            ("gone".to_string(), r"D:\vid\sub.mp4".to_string()),
            ("kept".to_string(), r"D:\vid\nas\pic.mp4".to_string()),
            ("elsewhere".to_string(), r"E:\vid\nas\pic.mp4".to_string()),
        ];
        let found = std::collections::HashSet::new();
        let skipped = vec![dir_prefix_lower(r"D:\vid\NAS")];

        let stale = compute_stale_ids(&existing, &dir_prefix_lower(r"D:\vid"), &found, &skipped);
        assert_eq!(stale, vec!["gone".to_string()]);
    }

    #[test]
    fn test_dir_prefix_lower_normalizes() {
        assert_eq!(dir_prefix_lower(r"D:\vid"), r"d:\vid\");
        assert_eq!(dir_prefix_lower(r"D:\vid\"), r"d:\vid\");
    }

    #[test]
    fn test_capped_walk_warnings() {
        let few = vec!["a".to_string(), "b".to_string()];
        assert_eq!(capped_walk_warnings(few.clone()), few);

        let many: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let capped = capped_walk_warnings(many);
        assert_eq!(capped.len(), 4);
        assert_eq!(capped[3], "另有 7 个目录未能读取");
    }

    fn library(count: usize) -> Vec<(String, String)> {
        (0..count).map(|i| (i.to_string(), format!(r"D:\vid\{}.mp4", i))).collect()
    }

    fn found_paths(count: usize) -> std::collections::HashSet<String> {
        (0..count)
            .map(|i| format!(r"d:\vid\keep{}.mp4", i))
            .collect()
    }

    /// 目录没读全时应拦住大批清理：要清的比扫到的还多，几乎不可能是用户真删了
    #[test]
    fn test_plan_stale_ids_blocks_mass_prune() {
        let existing = library(500);
        let mut warnings = Vec::new();
        let stale = plan_stale_ids(
            &existing,
            &dir_prefix_lower(r"D:\vid"),
            &found_paths(10),
            &[],
            &mut warnings,
        );
        assert!(stale.is_empty(), "疑似目录未读全，不该清理: {:?}", stale);
        assert_eq!(warnings.len(), 1);
    }

    /// 正常批量删除放行：留下的文件数与删掉的数量同量级
    #[test]
    fn test_plan_stale_ids_allows_bulk_delete() {
        let existing = library(500);
        let mut warnings = Vec::new();
        let stale = plan_stale_ids(
            &existing,
            &dir_prefix_lower(r"D:\vid"),
            &found_paths(5000),
            &[],
            &mut warnings,
        );
        assert_eq!(stale.len(), 500);
        assert!(warnings.is_empty());
    }

    /// 小批量删除哪怕目录已经空了也照清
    #[test]
    fn test_plan_stale_ids_allows_small_batch() {
        let existing = library(50);
        let mut warnings = Vec::new();
        let stale = plan_stale_ids(
            &existing,
            &dir_prefix_lower(r"D:\vid"),
            &found_paths(0),
            &[],
            &mut warnings,
        );
        assert_eq!(stale.len(), 50);
        assert!(warnings.is_empty());
    }

    /// 没读到的目录一多（整盘扫描常碰到权限目录），逐条比前缀不划算，直接放弃本轮清理
    #[test]
    fn test_plan_stale_ids_gives_up_when_many_skipped() {
        let existing = library(500);
        let skipped: Vec<std::path::PathBuf> = (0..25)
            .map(|i| std::path::PathBuf::from(format!(r"D:\vid\locked{}", i)))
            .collect();
        let mut warnings = Vec::new();
        let stale = plan_stale_ids(
            &existing,
            &dir_prefix_lower(r"D:\vid"),
            &found_paths(5000),
            &skipped,
            &mut warnings,
        );
        assert!(stale.is_empty());
        assert_eq!(warnings, vec!["有 25 个目录本轮未能读取，已跳过失效清理".to_string()]);
    }
}
