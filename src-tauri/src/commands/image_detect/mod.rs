use super::{read_cache, remove_cache, AppState, MapErrStr};
use super::{DUPLICATE_CACHE, VIDEO_DUPLICATE_CACHE};

mod detect_util;
mod duplicate;
mod template;

pub use duplicate::*;
pub use template::*;

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

// 缓存的文件名、原子落盘、读不回来当没有：这三件事两趟检测共用一套，实现在 commands.rs。

/// 缓存里记着"存这份结果时库里有多少条记录"，数量对不上就说明库变了，界面据此提示过期
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateCache {
    pub library_count: usize,
    pub report: DuplicateReport,
    /// 存这份结果时是否比对了像素：老缓存没这个字段，默认 true（当时只有这一种模式）
    #[serde(default = "default_pixel_verify")]
    pub pixel_verify: bool,
}

fn default_pixel_verify() -> bool {
    true
}

#[tauri::command]
pub async fn get_duplicate_cache(app: tauri::AppHandle) -> Result<Option<DuplicateCache>, String> {
    read_cache(&app, DUPLICATE_CACHE).await
}

/// 结果面板上按 ✕ 是"我不认这份结果"：缓存得跟着删，不然下次打开又给恢复回来
#[tauri::command]
pub async fn clear_detection_cache(app: tauri::AppHandle, which: String) -> Result<(), String> {
    let name = match which.as_str() {
        "duplicate" => DUPLICATE_CACHE,
        "videoDuplicate" => VIDEO_DUPLICATE_CACHE,
        other => return Err(format!("未知的检测结果：{other}")),
    };
    remove_cache(&app, name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::scanner;
    use crate::db;
    use crate::commands::workers;
    use super::detect_util::UnionFind;
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

    /// 32 档采样干涉的补救线：真实照片同图不同分辨率实测 32 档 mean=12.625、
    /// 96+ 档 mean≈0.2——旧 12.0 线把补救档直接堵死导致漏检。
    /// 构造 32 网格均差 13（过旧线、不过新 14 线），补救档给相同网格，必须判为同一张；
    /// 若有人把线调回 12（含）以下，这个测试会失败。
    #[test]
    fn test_rescue_grid_saves_32px_aliasing() {
        let px = |v: u8| vec![v; PIX_GRID * PIX_GRID];
        let mk = |id: &str, index: usize, v: u8| Candidate {
            index,
            id: id.to_string(),
            created_at: String::new(),
            pixels: px(v),
        };
        // 全 100 vs 全 113：每级差 13，mean=13，big=0（13<16），32 档判不同
        let (a, b) = (mk("a", 0, 100), mk("b", 1, 113));
        assert!(!pixels_close(&a.pixels, &b.pixels));
        // 补救档（96/192/256）给完全相同的网格：真副本在高档位是对得上的
        let rescue_same = vec![77u8; 96 * 96];
        let mut rescue = |index: usize, n: usize| {
            assert!([96, 192, 256].contains(&n), "补救档位不应新增");
            let _ = index;
            Some(rescue_same.clone())
        };
        assert!(close_against_with(&a, &b, &mut rescue));
        // 补救档也对不上（真不同的图：两边给不同网格）：还是判不同，放宽补救线不制造误判
        let mut rescue_miss = |index: usize, n: usize| {
            Some(vec![if index == 0 { 50u8 } else { 200u8 }; n * n])
        };
        assert!(!close_against_with(&a, &b, &mut rescue_miss));
    }

    /// 快检成组（不解像素）：候选池按入库时间直接成组，大组排前；像素核对的活儿留给面板人工复核
    #[test]
    fn test_group_pools_by_time_orders_by_created_at() {
        let row = |id: &str, created: &str| db::ImageSig {
            id: id.into(),
            path: format!("C:/pics/{id}.png"),
            created_at: created.into(),
            modified_at: None,
            phash: None,
            dhash: None,
            sig_pixels: None,
            sig_modified_at: None,
        };
        let rows = vec![
            row("late", "2026-03-01"),
            row("early", "2026-01-01"),
            row("mid", "2026-02-01"),
            row("solo-a", "2026-01-02"),
            row("solo-b", "2026-04-01"),
        ];
        // 池是无序下标，出来必须按时间排、最早的当保留原件；大组排前面
        let groups = group_pools_by_time(&rows, vec![vec![0, 2, 1], vec![3, 4]]);
        assert_eq!(groups[0], vec!["early".to_string(), "mid".to_string(), "late".to_string()]);
        assert_eq!(groups[1], vec!["solo-a".to_string(), "solo-b".to_string()]);
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
            // 子线程 panic 不带崩整单：失败分片按空计
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
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
    }
}
