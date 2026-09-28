use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, State};

use crate::commands::{flush, flush_with_retry, lock_ignoring_poison, workers, write_cache};
use super::{AppState, MapErrStr, DUPLICATE_CACHE};
use super::{DuplicateCache, DuplicateProgress, DuplicateReport, SigUpdate};
use crate::db;
use crate::scanner;

use super::detect_util::UnionFind;

/// 只对还缺双指纹的行跑 ffmpeg，攒一批落一次库并推一次进度。
/// 逐张 spawn ffmpeg 是这趟的全部代价，所以按线程分片（和相似检测同一套 14 线程上限）。
/// `event` 是进度事件名（重复检测与模板匹配共用这套补算，各推各的频道）。
/// 返回解不出图的张数。
fn ensure_sigs(
    app: &tauri::AppHandle,
    rows: &[db::ImageSig],
    indexes: &[usize],
    pairs: &mut [Option<(u64, u64)>],
    event: &'static str,
) -> usize {
    const EMIT_EVERY: usize = 256;
    const FLUSH_EVERY: usize = 64;
    let total = indexes.len();
    let _ = app.emit(event, DuplicateProgress { processed: 0, total, stage: "sigs", skipped: 0, groups: Vec::new() });
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
                            lock_ignoring_poison(hits).push((index, phash, dhash));
                            batch.push((row.id.clone(), phash as i64, dhash as i64, row.modified_at.clone()));
                        }
                        None => {
                            failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    if batch.len() >= FLUSH_EVERY {
                        let _ = flush(app, &mut batch, db::save_image_sigs);
                    }
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if processed % EMIT_EVERY == 0 || processed == total {
                        let _ = app.emit(
                            event,
                            DuplicateProgress { processed, total, stage: "sigs", skipped: failed.load(Ordering::Relaxed), groups: Vec::new() },
                        );
                    }
                }
                if !batch.is_empty() {
                    lock_ignoring_poison(pending).append(&mut batch);
                }
            });
        }
    });

    for (index, phash, dhash) in std::mem::take(hits.get_mut().unwrap_or_else(|e| e.into_inner())) {
        pairs[index] = Some((phash, dhash));
    }
    let mut leftover = std::mem::take(pending.get_mut().unwrap_or_else(|e| e.into_inner()));
    flush_with_retry(app, &mut leftover, db::save_image_sigs);
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
                            lock_ignoring_poison(collected).extend(fresh);
                        }
                    }
                    if batch.len() >= FLUSH_EVERY {
                        let _ = flush(app, &mut batch, db::save_image_pixels);
                    }
                    let processed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if processed % EMIT_EVERY == 0 || processed == total {
                        let groups = lock_ignoring_poison(collected).clone();
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
                    lock_ignoring_poison(pending).append(&mut batch);
                }
            });
        }
    });

    let mut leftover = std::mem::take(pending.get_mut().unwrap_or_else(|e| e.into_inner()));
    flush_with_retry(app, &mut leftover, db::save_image_pixels);
    let mut groups = std::mem::take(collected.get_mut().unwrap_or_else(|e| e.into_inner()));
    groups.sort_by_key(|group| std::cmp::Reverse(group.len()));
    (groups, skipped + failed.into_inner())
}

/// 存库当缓存的那一档边长
pub(crate) const PIX_GRID: usize = 32;
/// 判同门槛：整幅平均差 ≤ 2/255，且差过 16 级的像素不到 10%。
/// 门槛是量出来的：本机 2400 多个"双指纹完全相同"的候选桶里，p50 = 0.2、p90 = 0.74，
/// 也就是重存/转格式的抖动基本都在 1 以内；两张不同的图随便就差到十几，这条线留着一个数量级。
const PIX_MEAN_MAX: f64 = 2.0;
const PIX_BIG_SHARE: f64 = 0.10;
/// 32 档对不上时再试的三档。两张分辨率差得远的图缩到某个固定网格会撞上采样干涉——
/// 实测同一张图在 32/64 档差 8 级、在 16/128/192 档只差 0.5 级，多试一档就少漏一批。
/// 96/192 盖到 4 倍放大；8 倍放大在 96/192 都差 5 级上下、256 档差 1.84 过线，
/// 所以再挂一档 256。只有前一档对不上的候选才会走到下一档，代价只落在极少数条目上。
const PIX_RESCUE_GRIDS: [usize; 3] = [96, 192, 256];
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
pub(crate) fn pixels_close(a: &[u8], b: &[u8]) -> bool {
    match pixel_gap(a, b) {
        Some((mean, share)) => mean <= PIX_MEAN_MAX && share <= PIX_BIG_SHARE,
        None => false,
    }
}

/// 桶里的一个候选
pub(crate) struct Candidate {
    /// 在 rows 里的下标（现算高分辨率档时按它取路径）
    pub(crate) index: usize,
    pub(crate) id: String,
    pub(crate) created_at: String,
    pub(crate) pixels: Vec<u8>,
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

pub(crate) fn close_against(
    rows: &[db::ImageSig],
    cache: &mut HashMap<(usize, usize), Vec<u8>>,
    a: &Candidate,
    b: &Candidate,
) -> bool {
    if pixels_close(&a.pixels, &b.pixels) {
        return true;
    }
    // 差得太远的对不配花两次 ffmpeg：这一趟候选量是老口径的百倍，省掉的都是白省
    if pixel_gap(&a.pixels, &b.pixels).is_none_or(|(mean, _)| mean > RESCUE_MEAN_MAX) {
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
pub(crate) fn group_bucket(rows: &[db::ImageSig], mut pool: Vec<Candidate>) -> Vec<Vec<String>> {
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
/// 4 的线后来被两轮实测改写：
/// ① 同一幅图换分辨率重存（1358×1920 → 1448×2048），pHash 一位不差、dHash 差 5 位，
///    像素层 mean=0.93 明明是同一张——缩放采样正好翻动 dHash 的相邻梯度位，收到 5；
/// ② "不考虑分辨率"这轮拿真实照片扫了 1/8x~8x 的整条倍率带：pHash 全程 0 位不差，
///    dHash 最多差 6（1/32 倍即 42px 的缩略图），8 倍画质烂图差 3——5 的线把极端
///    缩略图挡在像素层外面，收到 7。
/// 代价可控：实库直方图每升一档只多几百个候选对（6→382 对、像素收下 84 对全是真重复），
/// 收不收仍由像素层说了算。边界：抖动随缩略图的绝对像素数涨——真实照片 42px 差 6、
/// 480p 的 1/32（15px）级别能差到 10，40px 以下的缩略图画面本身就糊成一片，不保。
pub(crate) const DUP_GATE: u32 = 7;

/// 用两枚感知哈希圈候选池：先做一次全量两两比对（和相似检测同一套按行分片，
/// 18.3 万枚实测 13 秒），再把"两路距离都 ≤ 门槛"的边并成池。
/// 池只是候选名单，最后仍由缩略像素定组——所以这里宁松勿紧，但不放过就等于不判。
// O(n²) 比对热路径：索引写法实测比迭代器 take(i) 快 2.3%（18.6 万枚 A/B），
// LLVM 对朴素索引循环的优化更好，风格让位给性能
#[allow(clippy::needless_range_loop)]
pub(crate) fn candidate_pools(pairs: &[Option<(u64, u64)>], gate: u32) -> Vec<Vec<usize>> {
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
            .flat_map(|h| h.join().unwrap())
            .collect()
    });

    let mut uf = UnionFind::new(total);
    for (a, b) in edges {
        uf.union(a as usize, b as usize);
    }
    let mut pools: HashMap<usize, Vec<usize>> = HashMap::new();
    for (rep, r) in reps.iter().enumerate() {
        pools.entry(uf.find(rep)).or_default().push(r.0);
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
        let conn = state.db.lock().map_err_str()?;
        db::get_image_sigs(&conn).map_err_str()?
    };
    let library_count = rows.len();

    let report = tauri::async_runtime::spawn_blocking(move || {
        // 文件已经不在磁盘上的条目不参与判定（逐文件 stat 不占主线程/数据库锁）
        rows.retain(|row| Path::new(&row.path).exists());
        let mut pairs: Vec<Option<(u64, u64)>> = rows.iter().map(|row| row.cached_sigs()).collect();
        let need_sigs: Vec<usize> = (0..rows.len()).filter(|&i| pairs[i].is_none()).collect();
        let skipped = ensure_sigs(&app, &rows, &need_sigs, pairs.as_mut_slice(), "duplicate-progress");

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
    .map_err_str()?;

    Ok(report)
}
