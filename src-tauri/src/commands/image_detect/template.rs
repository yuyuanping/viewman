use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, Manager, State};

use crate::commands::{flush, lock_ignoring_poison, workers};
use crate::db;
use crate::scanner;

use super::SigUpdate;
use super::{AppState, MapErrStr};

/// 模板匹配的一条命中：图片 id + 与模板的 PDQ 距离（256 位汉明距离，见 pdq.rs）
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

/// 命中距离上限（PDQ 256 位口径）：与前端滑杆上限一致（96）。
/// 256 位随机对的距离期望在 128 上下、几乎不会低于 96——上限卡在随机洪峰之前，
/// 与老 128 位求和口径的 48（也是随机带前沿）同一个卡法；滑杆之外的命中本来
/// 就显示不出来，不传。
pub(crate) const TEMPLATE_DISTANCE_MAX: u32 = 96;

/// 模板匹配的距离：PDQ 感知哈希的 256 位汉明距离（单一通道、粒度 0–256）。
/// 取代旧的"两枚 64 位哈希距离求和"——求和把两路等权混在一起，排序口径不单调；
/// PDQ 对重编码/缩放的稳健性是业界实测过的（Meta 威胁情报系统同款），
/// 裁剪/位移场景的召回也更好。收不收由人拖滑杆。
use crate::pdq::pdq_distance as template_pdq_distance;

/// 模板匹配的进度事件负载（事件名 template-progress）：只报补算指纹的张数
#[derive(Clone, serde::Serialize)]
struct TemplateProgress {
    processed: usize,
    total: usize,
}

/// 以一张图为模板找库里的相似图（以图搜图）：模板算一遍双指纹，和全库逐一比汉明距离。
/// 只做一趟一对一比对（18.6 万条毫秒级），指纹缺的现补（落库，下次就是零头）。
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
        let roots = crate::commands::settings::load_roots(
            &conn,
            crate::commands::settings::IMAGE_SCAN_ROOTS_KEY,
        );
        db::get_image_sigs_scoped(&conn, &roots).map_err_str()?
    };
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<TemplateMatchResult, String> {
            // 与相似检测同一口径：文件已不在磁盘上的不参与（逐文件 stat 挪进阻塞线程）
            rows.retain(|row| Path::new(&row.path).exists());
            let template_at = rows
                .iter()
                .position(|row| row.id == image_id)
                .ok_or_else(|| "模板图已不在库里（可能刚被删除或移出扫描范围）。".to_string())?;
            let mut pairs: Vec<Option<(u64, u64)>> =
                rows.iter().map(|row| row.cached_sigs()).collect();
            // 模板缺什么补什么：双指纹（32×32）与 PDQ（64×64）各一趟 ffmpeg，解完落库。
            // 缺 dhash 的旧行 cached_sigs 会整体当没有
            let mut t_phash_dhash = pairs[template_at];
            let mut t_pdq = rows[template_at].cached_pdq();
            if t_phash_dhash.is_none() || t_pdq.is_none() {
                let fp = scanner::image_fingerprints(&rows[template_at].path)
                    .ok_or_else(|| "模板图解不出画面，没法当模板。".to_string())?;
                if t_phash_dhash.is_none() {
                    t_phash_dhash = Some((fp.phash, fp.dhash));
                    pairs[template_at] = t_phash_dhash;
                }
                t_pdq = Some(fp.pdq);
                let row = &rows[template_at];
                if let Ok(conn) = app.state::<AppState>().db.lock() {
                    if let Some((phash, dhash)) = t_phash_dhash {
                        let _ = db::save_image_sigs(
                            &conn,
                            &[(
                                row.id.clone(),
                                phash as i64,
                                dhash as i64,
                                row.modified_at.clone(),
                            )],
                        );
                    }
                    let _ = db::save_image_pdq(&conn, &[(row.id.clone(), fp.pdq.to_vec())]);
                }
            }
            // 双指纹留作 pairs 缓存的一部分；比对只用 PDQ
            let _ = t_phash_dhash;
            let t_pdq = t_pdq.expect("上面刚补过");

            // 缺指纹的现补：复用重复检测那套按线程分片的补算（进度推到 template-progress 频道），
            // 补出来的直接拿进比对，落库留给下一趟当缓存
            let need: Vec<usize> = (0..rows.len())
                .filter(|&i| {
                    i != template_at && (pairs[i].is_none() || rows[i].cached_pdq().is_none())
                })
                .collect();
            let _ = app.emit(
                "template-progress",
                TemplateProgress {
                    processed: 0,
                    total: need.len(),
                },
            );
            let (fresh, skipped) = ensure_sigs_for_template(&app, &rows, &need);
            let mut fresh_pdq_by_id: HashMap<String, [u8; 32]> = fresh
                .into_iter()
                .map(|(id, _sigs, pdq)| (id, pdq))
                .collect();

            let mut matches: Vec<TemplateMatch> = Vec::new();
            for (index, row) in rows.iter().enumerate() {
                if index == template_at {
                    continue;
                }
                // 补算刚出来的优先；没补的（本轮已有缓存）从行上取
                let pdq = fresh_pdq_by_id.remove(&row.id).or_else(|| row.cached_pdq());
                // 解不出画面的进不了比对：返回值里带上张数，别让"没匹配"被当成定论
                if let Some(pdq) = pdq {
                    let dd = template_pdq_distance(&pdq, &t_pdq);
                    if dd <= TEMPLATE_DISTANCE_MAX {
                        matches.push(TemplateMatch {
                            id: row.id.clone(),
                            distance: dd,
                        });
                    }
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
/// 双指纹缺的补全套（32×32 + 64×64 两趟 ffmpeg），只缺 PDQ 的补 64×64 一趟。
/// fresh 里 Option 为 None 的行 = 双指纹本就有缓存、这次只算了 PDQ。
/// 单条补算结果：图片 id + 双指纹（本轮已缓存不重算的为 None）+ PDQ
type FreshSig = (String, Option<(u64, u64)>, [u8; 32]);

fn ensure_sigs_for_template(
    app: &tauri::AppHandle,
    rows: &[db::ImageSig],
    indexes: &[usize],
) -> (Vec<FreshSig>, usize) {
    const EMIT_EVERY: usize = 256;
    const FLUSH_EVERY: usize = 64;
    let total = indexes.len();
    let mut fresh = std::sync::Mutex::new(Vec::<FreshSig>::new());
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let threads = workers(total);

    std::thread::scope(|s| {
        for slot in 0..threads {
            let (fresh, done, failed) = (&fresh, &done, &failed);
            s.spawn(move || {
                let mut batch: Vec<SigUpdate> = Vec::with_capacity(FLUSH_EVERY);
                let mut pdq_batch: Vec<(String, Vec<u8>)> = Vec::with_capacity(FLUSH_EVERY);
                for (n, &index) in indexes.iter().enumerate() {
                    if n % threads != slot {
                        continue;
                    }
                    let row = &rows[index];
                    // 已有双指纹缓存的只补 PDQ（一趟 ffmpeg）；都没有的补全套
                    let outcome = if row.cached_sigs().is_some() {
                        scanner::image_pdq(&row.path).map(|pdq| (None, pdq))
                    } else {
                        scanner::image_fingerprints(&row.path)
                            .map(|fp| (Some((fp.phash, fp.dhash)), fp.pdq))
                    };
                    match outcome {
                        Some((sigs, pdq)) => {
                            if let Some((phash, dhash)) = sigs {
                                batch.push((
                                    row.id.clone(),
                                    phash as i64,
                                    dhash as i64,
                                    row.modified_at.clone(),
                                ));
                            }
                            pdq_batch.push((row.id.clone(), pdq.to_vec()));
                            lock_ignoring_poison(fresh).push((row.id.clone(), sigs, pdq));
                        }
                        None => {
                            failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    if batch.len() >= FLUSH_EVERY {
                        let _ = flush(app, &mut batch, db::save_image_sigs);
                    }
                    if pdq_batch.len() >= FLUSH_EVERY {
                        let _ = flush(app, &mut pdq_batch, db::save_image_pdq);
                    }
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if processed % EMIT_EVERY == 0 || processed == total {
                        let _ =
                            app.emit("template-progress", TemplateProgress { processed, total });
                    }
                }
                if !batch.is_empty() {
                    let _ = flush(app, &mut batch, db::save_image_sigs);
                }
                if !pdq_batch.is_empty() {
                    let _ = flush(app, &mut pdq_batch, db::save_image_pdq);
                }
            });
        }
    });

    let fresh = std::mem::take(fresh.get_mut().unwrap_or_else(|e| e.into_inner()));
    (fresh, failed.into_inner())
}
