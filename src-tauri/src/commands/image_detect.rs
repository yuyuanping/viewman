use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use tauri::{Emitter, Manager, State};

use crate::commands::{flush, flush_with_retry, lock_ignoring_poison, workers};
use crate::db;
use crate::scanner;

use super::{read_cache, remove_cache, write_cache, AppState, MapErrStr};
use super::{DUPLICATE_CACHE, SIMILAR_CACHE, VIDEO_DUPLICATE_CACHE};

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

// flush / flush_with_retry / workers / lock_ignoring_poison 已提到 commands.rs 共享

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
    let Some(mut cached): Option<SimilarCache> = read_cache(&app, SIMILAR_CACHE).await? else {
        return Ok(None);
    };
    if cached.result.groups.is_empty() {
        return Ok(Some(cached));
    }
    // 旧缓存可能是照着全库算的，会挂着扫描范围外的组员：恢复前按当前扫描根裁一遍，
    // 跟现跑的口径一致，否则面板里会出现图片库看不到的图。裁到不足两张的组不再算一组。
    // 裁剪要全表 LIKE 扫描 + 十几万 id 进集合，扔进阻塞线程池；这是启动恢复路径，别占主线程
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err_str()?;
        let roots = super::settings::load_roots(&conn, super::settings::IMAGE_SCAN_ROOTS_KEY);
        // 无扫描根 = 全库都在范围内，裁剪是恒等操作，别白跑一遍全表查询
        if !roots.is_empty() {
            let scoped: HashSet<String> = db::image_ids_in_scope(&conn, &roots)
                .map_err_str()?
                .into_iter()
                .collect();
            cached.result.groups = cached
                .result
                .groups
                .into_iter()
                .map(|group| group.into_iter().filter(|id| scoped.contains(id)).collect::<Vec<_>>())
                .filter(|group| group.len() > 1)
                .collect();
            cached.result.far.retain(|id| scoped.contains(id));
            cached.result.hashes.retain(|hit| scoped.contains(&hit.id));
        }
        Ok(Some(cached))
    })
    .await
    .map_err_str()?
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
const PIX_GRID: usize = 32;
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
/// 4 的线后来被两轮实测改写：
/// ① 同一幅图换分辨率重存（1358×1920 → 1448×2048），pHash 一位不差、dHash 差 5 位，
///    像素层 mean=0.93 明明是同一张——缩放采样正好翻动 dHash 的相邻梯度位，收到 5；
/// ② "不考虑分辨率"这轮拿真实照片扫了 1/8x~8x 的整条倍率带：pHash 全程 0 位不差，
///    dHash 最多差 6（1/32 倍即 42px 的缩略图），8 倍画质烂图差 3——5 的线把极端
///    缩略图挡在像素层外面，收到 7。
/// 代价可控：实库直方图每升一档只多几百个候选对（6→382 对、像素收下 84 对全是真重复），
/// 收不收仍由像素层说了算。边界：抖动随缩略图的绝对像素数涨——真实照片 42px 差 6、
/// 480p 的 1/32（15px）级别能差到 10，40px 以下的缩略图画面本身就糊成一片，不保。
const DUP_GATE: u32 = 7;

/// 用两枚感知哈希圈候选池：先做一次全量两两比对（和相似检测同一套按行分片，
/// 18.3 万枚实测 13 秒），再把"两路距离都 ≤ 门槛"的边并成池。
/// 池只是候选名单，最后仍由缩略像素定组——所以这里宁松勿紧，但不放过就等于不判。
// O(n²) 比对热路径：索引写法实测比迭代器 take(i) 快 2.3%（18.6 万枚 A/B），
// LLVM 对朴素索引循环的优化更好，风格让位给性能
#[allow(clippy::needless_range_loop)]
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
    rows.retain(|row| Path::new(&row.path).exists());

    let report = tauri::async_runtime::spawn_blocking(move || {
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
    // 同 candidate_pools：O(n²) 热路径，索引写法实测快 2.3%
    #[allow(clippy::needless_range_loop)]
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
const TEMPLATE_DISTANCE_MAX: u32 = 48;

/// 模板匹配的距离：两枚感知哈希的汉明距离**求和**，不取大。
/// 取大是相似分组那边的从严口径——分组宁可漏不掉；以图搜图要的是召回，
/// 一路指纹漂远（缩放/裁剪/重编码专门动 dHash 的相邻梯度）不该把整对否掉：
/// 实测重存副本 dHash 就能漂 5~10 位，极端缩略图到 10+，取大时模板只认出
/// pHash 也近的那一小半，求和后另一路够近就还有机会进清单，收不收由人拖滑杆。
fn template_distance(phash: u64, dhash: u64, t_phash: u64, t_dhash: u64) -> u32 {
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
        let roots = super::settings::load_roots(&conn, super::settings::IMAGE_SCAN_ROOTS_KEY);
        db::get_image_sigs_scoped(&conn, &roots).map_err_str()?
    };
    rows.retain(|row| Path::new(&row.path).exists());

    let result = tauri::async_runtime::spawn_blocking(move || -> Result<TemplateMatchResult, String> {
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

    // 只比对扫描范围内的图：范围外的记录（如被移出根的落点）不进聚类，
    // 与图片库视图同一口径。缓存的过期判断按"库里的记录数"，所以要在过滤之后数
    let mut rows: Vec<db::ImageSig> = {
        let conn = state.db.lock().map_err_str()?;
        let roots = super::settings::load_roots(&conn, super::settings::IMAGE_SCAN_ROOTS_KEY);
        db::get_image_sigs_scoped(&conn, &roots).map_err_str()?
    };
    let library_count = rows.len();
    // 文件已经不在磁盘上的条目不参与聚类，也不用再白跑一遍解码
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
            if processed.is_multiple_of(FLUSH_EVERY) || processed == total {
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
    .map_err_str()?;

    Ok(result)
}


#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(!pixels_close(&grid(10), &[10; 4]));
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

    /// 分辨率差异不再把重复图拆开（"不考虑分辨率"）：1/16 倍缩略图靠门槛放宽收进来，
    /// 8 倍放大的 32 档采样干涉靠补救档——两条路都走真实的 close_against 链路
    #[test]
    fn test_resolution_differences_still_match() {
        if !scanner::ffmpeg_available() {
            eprintln!("跳过：本机未安装 ffmpeg");
            return;
        }
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("viewman_res_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let render = |name: &str, size: &str, quality: &[&str]| {
            let out = dir.join(name);
            let ok = Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", "testsrc2=size=480x360:rate=10:duration=1", "-frames:v", "1"])
                .args(["-vf", &format!("scale={size}:flags=bicubic")])
                .args(quality)
                .arg(&out)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "生成测试图片失败");
            out.to_string_lossy().to_string()
        };
        // 底图 480×360；1/16 倍即 30px、1/8 倍即 60px 的缩略图，8 倍即 3840×2700。
        // 30px 只用来量哈希抖动；像素核对用 60px——比它更小的缩略图（15px 级别）
        // 连画面信息都快没了，合成图案下每个网格档都对不上，不属"同一张图的两份拷贝"
        let original = render("orig.png", "480x360", &[]);
        let thumbnail = render("thumb.jpg", "60:-2", &["-q:v", "3"]);
        let upscaled = render("big.jpg", "3840:2700", &["-q:v", "3"]);

        // 门槛放宽量的是哈希抖动：极小缩略图的 dHash 差距要仍在 DUP_GATE 内
        let (_, d_tiny) = scanner::image_hashes(&render("tiny.jpg", "30:-2", &["-q:v", "3"])).unwrap();
        let (_, d_base) = scanner::image_hashes(&original).unwrap();
        assert!(
            (d_tiny ^ d_base).count_ones() <= DUP_GATE,
            "1/16 倍缩略图的 dHash 抖动超出门槛，候选阶段就会漏"
        );

        let rows = vec![
            db::ImageSig { id: "a".into(), path: original.clone(), created_at: String::new(), modified_at: None, phash: None, dhash: None, sig_pixels: None, sig_modified_at: None },
            db::ImageSig { id: "b".into(), path: thumbnail.clone(), created_at: String::new(), modified_at: None, phash: None, dhash: None, sig_pixels: None, sig_modified_at: None },
            db::ImageSig { id: "c".into(), path: upscaled.clone(), created_at: String::new(), modified_at: None, phash: None, dhash: None, sig_pixels: None, sig_modified_at: None },
        ];
        let candidate = |index: usize, path: &str| Candidate {
            index,
            id: String::new(),
            created_at: String::new(),
            pixels: scanner::gray_pixels(path, PIX_GRID).unwrap(),
        };
        let mut cache: HashMap<(usize, usize), Vec<u8>> = HashMap::new();
        let base = candidate(0, &original);
        for (index, path) in [(1usize, thumbnail.as_str()), (2usize, upscaled.as_str())] {
            assert!(
                close_against(&rows, &mut cache, &base, &candidate(index, path)),
                "{path} 与原图仅分辨率不同，应判为同一张"
            );
        }
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

    /// 确定性伪随机（xorshift64*）：两次跑拿到完全同一批指纹，A/B 才可比
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// candidate_pools 的迭代器写法复刻（clippy 939cb85 引入的形态），
    /// 生产已回退为索引写法，此函数留作性能对照，逻辑与当时一字不差。
    #[allow(clippy::needless_range_loop)]
    fn candidate_pools_iterator_form(pairs: &[Option<(u64, u64)>], gate: u32) -> Vec<Vec<usize>> {
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
            handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
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

    /// 模板匹配的求和口径：一路指纹漂很远、另一路很近的副本要能进清单——
    /// 这正是"取大"规则匹配不到的那类图
    #[test]
    fn test_template_distance_sums_both_hashes() {
        // dHash 漂 30 位、pHash 只差 2：取大是 30（被否），求和 32（进清单）
        assert_eq!(template_distance(0b11, (1u64 << 30) - 1, 0, 0), 30 + 2);
        // 两路都远的真不同图，求和照样把它挡在外面
        assert!(template_distance(0xFFFF_FFFF_FFFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF, 0, 0) > TEMPLATE_DISTANCE_MAX);
    }

    /// 分组清单归一化（组内组间都排序），供新旧实现比对等价性
    fn norm_pools(mut pools: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        for pool in &mut pools {
            pool.sort_unstable();
        }
        pools.sort();
        pools
    }

    /// scan 内层比对循环的两种写法复刻（单线程，隔离多线程噪声）：
    /// indexed = 索引式 `for j in 0..i`（生产写法），iterator = 迭代器式 `enumerate().take(i)`
    /// （clippy 曾改写成后者，18.6 万枚实测慢 2.3% 后回退——此对照留作该 allow 的依据）
    #[allow(clippy::needless_range_loop)] // 对照组就是索引写法，改了就测不到东西
    #[inline(never)]
    fn loop_indexed(hashes: &[(u64, u64)], threshold: u32) -> usize {
        let total = hashes.len();
        let mut edges = 0usize;
        for i in 1..total {
            let (pi, di) = hashes[i];
            for j in 0..i {
                let (pj, dj) = hashes[j];
                if (pi ^ pj).count_ones().max((di ^ dj).count_ones()) <= threshold {
                    edges += 1;
                }
            }
        }
        edges
    }

    #[inline(never)]
    fn loop_iterator(hashes: &[(u64, u64)], threshold: u32) -> usize {
        let total = hashes.len();
        let mut edges = 0usize;
        let mut i = 1;
        while i < total {
            let (pi, di) = hashes[i];
            for (j, &(pj, dj)) in hashes.iter().enumerate().take(i) {
                let _ = j;
                if (pi ^ pj).count_ones().max((di ^ dj).count_ones()) <= threshold {
                    edges += 1;
                }
            }
            i += 1;
        }
        edges
    }

    /// 性能实测：18.6 万枚指纹（历史基线同规模）。手动跑，不进常规回归：
    /// `cargo test --release perf_detection_186k -- --ignored --nocapture`（bash 下先 `ulimit -s 65536`）
    #[test]
    #[ignore]
    fn perf_detection_186k() {
        use std::time::Instant;
        const N: usize = 186_000;
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);

        // 前 90% 孤张（双 64 位随机串），后 10% 挂在孤张上各翻 1~3 位：
        // 随机对落在 gate 内的概率可忽略，所以边几乎全部来自簇——贴近真库"大量孤张+少量重复簇"
        let mut hashes: Vec<(u64, u64)> = vec![(0, 0); N];
        let pure = N * 9 / 10;
        for h in hashes.iter_mut().take(pure) {
            *h = (rng.next(), rng.next());
        }
        for i in pure..N {
            let (mut p, mut d) = hashes[rng.below(pure as u64) as usize];
            for _ in 0..rng.below(3) + 1 {
                p ^= 1 << rng.below(64);
            }
            for _ in 0..rng.below(3) + 1 {
                d ^= 1 << rng.below(64);
            }
            hashes[i] = (p, d);
        }
        let pairs: Vec<Option<(u64, u64)>> = hashes.iter().copied().map(Some).collect();

        // ── A/B 1：candidate_pools 全链路（多线程 + 比对 + 并查集），交替各跑两次取最小 ──
        let (mut t_new, mut t_old) = (std::time::Duration::MAX, std::time::Duration::MAX);
        let mut pools_new = Vec::new();
        let mut pools_old = Vec::new();
        for round in 0..3 {
            let t = Instant::now();
            let p = candidate_pools(&pairs, DUP_GATE);
            let d = t.elapsed();
            if round == 0 || d < t_new {
                t_new = d;
                pools_new = p;
            }
            let t = Instant::now();
            let p = candidate_pools_iterator_form(&pairs, DUP_GATE);
            let d = t.elapsed();
            if round == 0 || d < t_old {
                t_old = d;
                pools_old = p;
            }
            let _ = round;
        }
        assert_eq!(
            norm_pools(pools_new.clone()),
            norm_pools(pools_old.clone()),
            "新旧 candidate_pools 分组结果必须一致"
        );
        // t_new = 索引(生产)，t_old = 迭代器：正值表示迭代器比生产慢
        let speedup = (t_old.as_secs_f64() / t_new.as_secs_f64() - 1.0) * 100.0;
        println!(
            "[perf] candidate_pools N={N} workers={}: 索引(生产) {:.2?} | 迭代器 {:.2?} | 迭代器写法 {:+.1}%",
            workers(N),
            t_new,
            t_old,
            speedup
        );
        println!(
            "[perf] 候选池 {} 个，共 {} 张进入像素层（占比 {:.1}%）",
            pools_new.len(),
            pools_new.iter().map(|p| p.len()).sum::<usize>(),
            pools_new.iter().map(|p| p.len()).sum::<usize>() as f64 / N as f64 * 100.0
        );

        // ── A/B 2：scan 内层循环两种写法，单线程隔离线程调度噪声，交替各跑三次取最小 ──
        // 随机孤张在阈值 10 下零边，纯随机测不出东西——每 1000 行植入第 0 行的近副本保底成边
        let mut probe = hashes[..60_000].to_vec();
        let (seed_p, seed_d) = probe[0];
        for i in (1000..60_000).step_by(1000) {
            let (mut p, mut d) = (seed_p, seed_d);
            p ^= 1 << rng.below(8);
            d ^= 1 << rng.below(8);
            probe[i] = (p, d);
        }
        let probe = probe.as_slice();
        assert_eq!(
            loop_indexed(probe, 10),
            loop_iterator(probe, 10),
            "两种循环写法产出的边数必须一致"
        );
        let mut it_t = std::time::Duration::MAX;
        let mut ix_t = std::time::Duration::MAX;
        for round in 0..3 {
            let t = Instant::now();
            let n = loop_iterator(probe, 10);
            let d = t.elapsed();
            if round == 0 || d < it_t {
                it_t = d;
            }
            assert!(n > 0);
            let t = Instant::now();
            let n = loop_indexed(probe, 10);
            let d = t.elapsed();
            if round == 0 || d < ix_t {
                ix_t = d;
            }
            assert!(n > 0);
        }
        println!(
            "[perf] scan 内层循环(单线程, N=6万): 索引(生产) {:.2?} | 迭代器 {:.2?} | 迭代器写法 {:+.1}%",
            ix_t,
            it_t,
            (it_t.as_secs_f64() / ix_t.as_secs_f64() - 1.0) * 100.0
        );

        // ── 端到端：SimilarGraph add + scan + partition，对照历史基线 11.6 秒（同为 14 线程）──
        let ids: Vec<String> = (0..N).map(|i| format!("img-{i:06}")).collect();
        let created: Vec<String> = (0..N).map(|i| format!("2026-01-01T00:{:02}:{:02}", i / 3600 % 60, i / 60 % 60)).collect();
        let mut graph = SimilarGraph::new(10);
        let t = Instant::now();
        for i in 0..N {
            graph.add(ids[i].as_str(), created[i].as_str(), hashes[i].0, hashes[i].1);
        }
        let t_add = t.elapsed();
        let t = Instant::now();
        graph.scan();
        let t_scan = t.elapsed();
        let t = Instant::now();
        let part = graph.partition();
        let t_part = t.elapsed();
        let grouped: usize = part.groups.iter().map(|g| g.len()).sum();
        println!(
            "[perf] SimilarGraph(14线程, N={N}): add {:.2?} | scan {:.2?}（历史基线 11.6s）| partition {:.2?}",
            t_add, t_scan, t_part
        );
        println!(
            "[perf] 分组 {} 个 / 成组 {} 张 / 远亲 {} 张",
            part.groups.len(),
            grouped,
            part.far.len()
        );
        assert!(!part.groups.is_empty(), "10% 簇数据应当成组");
    }
}
