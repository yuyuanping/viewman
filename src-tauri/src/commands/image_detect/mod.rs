use std::collections::HashSet;

use tauri::Manager;

use crate::db;

use super::{read_cache, remove_cache, AppState, MapErrStr};
use super::{DUPLICATE_CACHE, SIMILAR_CACHE, VIDEO_DUPLICATE_CACHE};

mod detect_util;
mod duplicate;
mod similar;
mod template;

pub use duplicate::*;
pub use similar::*;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::scanner;
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
