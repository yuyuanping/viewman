use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, Manager, State};

use crate::commands::{flush, lock_ignoring_poison, workers};
use crate::db;
use crate::scanner;

use super::{AppState, MapErrStr};
use super::SigUpdate;

/// 模板匹配的一条命中：图片 id + 与模板的指纹距离
/// （两枚感知哈希的距离**求和**，见 template_distance）
#[derive(Clone, serde::Serialize)]
pub struct TemplateMatch {
    pub id: String,
    pub distance: u32,
}

/// 模板匹配结论：按距离升序的命中清单 + 解不出指纹而没参与比对的张数
#[derive(Clone, serde::Serialize)]
pub struct TemplateMatchResult {
    pub matches: Vec<TemplateMatch>,
    pub skipped: usize,
}

/// 命中距离上限：与前端滑杆上限一致（48）。曾经想放到 64"让前端自己收"，
/// 实测 sum≤64 有 11.4 万条命中——返回值白扛 6 MB、元数据再现查 45 MB，
/// 纯属把洪峰搬过 IPC；滑杆之外的命中本来就显示不出来，不传。
pub(crate) const TEMPLATE_DISTANCE_MAX: u32 = 48;

/// 模板匹配的距离：两枚感知哈希的汉明距离**求和**，不取大。
/// 取大是相似分组那边的从严口径——分组宁可漏不掉；以图搜图要的是召回，
/// 一路指纹漂远（缩放/裁剪/重编码专门动 dHash 的相邻梯度）不该把整对否掉：
/// 实测重存副本 dHash 就能漂 5~10 位，极端缩略图到 10+，取大时模板只认出
/// pHash 也近的那一小半，求和后另一路够近就还有机会进清单，收不收由人拖滑杆。
pub(crate) fn template_distance(phash: u64, dhash: u64, t_phash: u64, t_dhash: u64) -> u32 {
    (phash ^ t_phash).count_ones() + (dhash ^ t_dhash).count_ones()
}

/// 模板匹配的进度事件负载（事件名 template-progress）：只报补算指纹的张数
#[derive(Clone, serde::Serialize)]
struct TemplateProgress {
    processed: usize,
    total: usize,
}

/// 以一张图为模板找库里的相似图（以图搜图）：模板算一遍双指纹，和全库逐一比汉明距离。
/// 与 find_similar_images 的差别是方向反过来了——不是"谁跟谁成团"，而是"谁跟这张像"，
/// 所以只做一趟一对一比对（18.6 万条毫秒级），指纹缺的现补（落库，下次就是零头）。
/// 模板自己不进结果；同图副本距离 0，是最先该看到的那批。
#[tauri::command]
pub async fn find_images_like_template(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    image_id: String,
) -> Result<TemplateMatchResult, String> {
    if !crate::scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法计算图片指纹。请安装 ffmpeg 并加入 PATH。".into());
    }
    // 与相似检测同一口径：只比扫描范围内的图，文件已不在磁盘上的不参与
    let mut rows: Vec<db::ImageSig> = {
        let conn = state.db.lock().map_err_str()?;
        let roots = crate::commands::settings::load_roots(&conn, crate::commands::settings::IMAGE_SCAN_ROOTS_KEY);
        db::get_image_sigs_scoped(&conn, &roots).map_err_str()?
    };
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<TemplateMatchResult, String> {
        // 与相似检测同一口径：文件已不在磁盘上的不参与（逐文件 stat 挪进阻塞线程）
        rows.retain(|row| Path::new(&row.path).exists());
        let template_at = rows
            .iter()
            .position(|row| row.id == image_id)
            .ok_or_else(|| "模板图已不在库里（可能刚被删除或移出扫描范围）。".to_string())?;
        let mut pairs: Vec<Option<(u64, u64)>> = rows.iter().map(|row| row.cached_sigs()).collect();
        // 模板没缓存指纹就先给自己算一份；缺 dhash 的旧行 cached_sigs 会整体当没有
        if pairs[template_at].is_none() {
            let (phash, dhash) = scanner::image_hashes(&rows[template_at].path)
                .ok_or_else(|| "模板图解不出画面，没法当模板。".to_string())?;
            pairs[template_at] = Some((phash, dhash));
            let row = &rows[template_at];
            if let Ok(conn) = app.state::<AppState>().db.lock() {
                let _ = db::save_image_sigs(
                    &conn,
                    &[(row.id.clone(), phash as i64, dhash as i64, row.modified_at.clone())],
                );
            }
        }
        let (t_phash, t_dhash) = pairs[template_at].unwrap();

        // 缺指纹的现补：复用重复检测那套按线程分片的补算（进度推到 template-progress 频道），
        // 补出来的直接拿进比对，落库留给下一趟当缓存
        let need: Vec<usize> = (0..rows.len())
            .filter(|&i| i != template_at && pairs[i].is_none())
            .collect();
        let _ = app.emit("template-progress", TemplateProgress { processed: 0, total: need.len() });
        let (fresh, skipped) = ensure_sigs_for_template(&app, &rows, &need);
        let mut fresh_by_id: HashMap<String, (u64, u64)> =
            fresh.into_iter().map(|(id, p, d)| (id, (p, d))).collect();

        let mut matches: Vec<TemplateMatch> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            if index == template_at {
                continue;
            }
            let sigs = fresh_by_id.remove(&row.id).or_else(|| pairs[index]);
            match sigs {
                Some((phash, dhash)) => {
                    let dd = template_distance(phash, dhash, t_phash, t_dhash);
                    if dd <= TEMPLATE_DISTANCE_MAX {
                        matches.push(TemplateMatch { id: row.id.clone(), distance: dd });
                    }
                }
                // 解不出画面的进不了比对：返回值里带上张数，别让"没匹配"被当成定论
                None => {}
            }
        }
        matches.sort_by(|a, b| a.distance.cmp(&b.distance).then_with(|| a.id.cmp(&b.id)));
        Ok(TemplateMatchResult { matches, skipped })
    })
    .await
    .map_err_str()??;

    Ok(result)
}

/// 模板匹配里补缺失指纹：与 ensure_sigs 同一套线程分片和落库节奏，
/// 但不回填 pairs（调用方只需要"这次算出了什么"），返回 (新指纹清单, 解不出图的张数)。
fn ensure_sigs_for_template(
    app: &tauri::AppHandle,
    rows: &[db::ImageSig],
    indexes: &[usize],
) -> (Vec<(String, u64, u64)>, usize) {
    const EMIT_EVERY: usize = 256;
    const FLUSH_EVERY: usize = 64;
    let total = indexes.len();
    let mut fresh = std::sync::Mutex::new(Vec::<(String, u64, u64)>::new());
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let threads = workers(total);

    std::thread::scope(|s| {
        for slot in 0..threads {
            let (fresh, done, failed) = (&fresh, &done, &failed);
            s.spawn(move || {
                let mut batch: Vec<SigUpdate> = Vec::with_capacity(FLUSH_EVERY);
                for (n, &index) in indexes.iter().enumerate() {
                    if n % threads != slot {
                        continue;
                    }
                    let row = &rows[index];
                    if let Some((phash, dhash)) = scanner::image_hashes(&row.path) {
                        lock_ignoring_poison(fresh).push((row.id.clone(), phash, dhash));
                        batch.push((row.id.clone(), phash as i64, dhash as i64, row.modified_at.clone()));
                    } else {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                    if batch.len() >= FLUSH_EVERY {
                        let _ = flush(app, &mut batch, db::save_image_sigs);
                    }
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if processed % EMIT_EVERY == 0 || processed == total {
                        let _ = app.emit("template-progress", TemplateProgress { processed, total });
                    }
                }
                if !batch.is_empty() {
                    let _ = flush(app, &mut batch, db::save_image_sigs);
                }
            });
        }
    });

    let fresh = std::mem::take(fresh.get_mut().unwrap_or_else(|e| e.into_inner()));
    (fresh, failed.into_inner())
}
