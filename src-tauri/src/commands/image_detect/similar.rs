use std::collections::{HashMap, HashSet};
use std::path::Path;

use tauri::{Emitter, Manager, State};

use crate::commands::write_cache;
use crate::db;
use crate::scanner;

use super::detect_util::UnionFind;
use super::{AppState, MapErrStr, SIMILAR_CACHE};
use super::SimilarCache;

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
pub(crate) struct SimilarPartition {
    pub(crate) groups: Vec<Vec<usize>>,
    pub(crate) far: HashSet<usize>,
}

const NO_GROUP: usize = usize::MAX;

/// 双指纹比对图。节点是"代表"（两枚指纹完全相同的只留一个，同图必同组），
/// 边是"两路汉明距离都 ≤ 阈值"，成组看的是互为前 2 近邻的骨架边。
pub(crate) struct SimilarGraph<'a> {
    threshold: u32,
    items: Vec<SigItem<'a>>,
    pub(crate) reps: Vec<usize>,
    rep_of: HashMap<(u64, u64), usize>,
    /// 代表 → 它承载的全部图片下标（含它自己）
    twins: Vec<Vec<usize>>,
    /// 代表 → 按 (距离, 另一端) 排好序的邻居表，只收 ≤ 阈值的
    nbrs: Vec<Vec<(u32, u32)>>,
    /// 已经比对过的代表数，用于增量补边
    scanned: usize,
}

impl<'a> SimilarGraph<'a> {
    pub(crate) fn new(threshold: u32) -> Self {
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
    pub(crate) fn add(&mut self, id: &'a str, created: &'a str, phash: u64, dhash: u64) {
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
    pub(crate) fn scan(&mut self) {
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

    pub(crate) fn partition(&self) -> SimilarPartition {
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

    pub(crate) fn member_ids(&self, partition: &SimilarPartition) -> Vec<Vec<String>> {
        partition
            .groups
            .iter()
            .map(|items| items.iter().map(|&item| self.items[item].id.to_string()).collect())
            .collect()
    }

    pub(crate) fn far_ids(&self, partition: &SimilarPartition) -> Vec<String> {
        let mut ids: Vec<usize> = partition.far.iter().copied().collect();
        ids.sort_unstable();
        ids.into_iter().map(|item| self.items[item].id.to_string()).collect()
    }

    /// 成组图片的指纹（孤张不给，免得 19 万条清单白传一趟）
    pub(crate) fn signatures(&self, partition: &SimilarPartition) -> Vec<SimilarHit> {
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

    // 只比对扫描范围内的图：范围外的记录（如被移出根的落点）不进聚类，
    // 与图片库视图同一口径。缓存的过期判断按"库里的记录数"，所以要在过滤之后数
    let mut rows: Vec<db::ImageSig> = {
        let conn = state.db.lock().map_err_str()?;
        let roots = crate::commands::settings::load_roots(&conn, crate::commands::settings::IMAGE_SCAN_ROOTS_KEY);
        db::get_image_sigs_scoped(&conn, &roots).map_err_str()?
    };
    let library_count = rows.len();

    let result = tauri::async_runtime::spawn_blocking(move || -> SimilarResult {
        // 文件已经不在磁盘上的条目不参与聚类，也不用再白跑一遍解码（逐文件 stat 不占主线程）
        rows.retain(|row| Path::new(&row.path).exists());
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
