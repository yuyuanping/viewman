use std::path::Path;

use tauri::{Emitter, Manager, State};

use crate::db;
use crate::scanner;

use super::{flush, flush_with_retry, read_cache, write_cache, AppState, MapErrStr, VIDEO_DUPLICATE_CACHE};

/// 单个采样条目：路径、待比对 id、三处采样帧的 (phash, dhash)。
type FrameEntry = (String, String, [(u64, u64); 3]);

/// 重复视频的进度事件负载（事件名 video-duplicate-progress）。
/// 两趟各报一次：`anchor` 补算锚点帧指纹，`verify` 给候选补抽中段/结尾两处复核帧。
#[derive(Clone, serde::Serialize)]
struct DuplicateProgress {
    processed: usize,
    total: usize,
    stage: &'static str,
}

/// 每个采样点的双指纹允许的最大汉明距离（pHash、dHash 各自都要过这条线）。
/// 视频重压制会实打实改动像素，不能像图片那样要求指纹全等；但锚点、中段、结尾
/// 三处都得同时过线，单点放宽到 6 在三处叠起来之后误判概率已经很低。
const DUP_FRAME_DIST: u32 = 6;

/// 某一处采样点是否算"同一幅画面"
fn frames_match(a: (u64, u64), b: (u64, u64)) -> bool {
    scanner::hamming_distance(a.0, b.0) <= DUP_FRAME_DIST
        && scanner::hamming_distance(a.1, b.1) <= DUP_FRAME_DIST
}

/// 并查集分桶：把下标两两按 `same` 并组，返回每组下标。
/// 这里只用来圈候选，链式并组（A≈B、B≈C 但 A≉C）不要紧——
/// 真正定组的是后面拿三处采样点逐一核对的那一趟。
fn cluster_by(len: usize, same: impl Fn(usize, usize) -> bool) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..len).collect();
    fn find(parent: &mut Vec<usize>, i: usize) -> usize {
        if parent[i] != i {
            let root = find(parent, parent[i]);
            parent[i] = root;
        }
        parent[i]
    }
    for a in 0..len {
        for b in (a + 1)..len {
            if same(a, b) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra] = rb;
                }
            }
        }
    }
    let mut buckets: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..len {
        buckets.entry(find(&mut parent, i)).or_default().push(i);
    }
    buckets.into_values().filter(|bucket| bucket.len() > 1).collect()
}

/// 以"最早添加的那条"为保留原件，逐个拿三处采样点核对：过线的进组，
/// 过不了线的留给下一轮另起一组（所以一组内任意两条都是直接比对过的）。
fn group_by_frames(entries: &[FrameEntry]) -> Vec<Vec<String>> {
    let mut pool: Vec<FrameEntry> = entries.to_vec();
    pool.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mut groups: Vec<Vec<String>> = Vec::new();
    while !pool.is_empty() {
        let keeper = pool.remove(0);
        let mut group = vec![keeper.0.clone()];
        pool.retain(|entry| {
            let matched = (0..3).all(|slot| frames_match(keeper.2[slot], entry.2[slot]));
            if matched {
                group.push(entry.0.clone());
            }
            !matched
        });
        if group.len() > 1 {
            groups.push(group);
        }
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.len()));
    groups
}

/// 只对还缺锚点帧的行跑 ffmpeg，攒一批落一次库并推一次进度。
/// 锚点统一从缩略图派生（32×32 缩略是同一条口径），没有缩略图的先补一张。
fn ensure_anchors(
    app: &tauri::AppHandle,
    rows: &[(db::VideoSig, String)],
    indexes: &[usize],
    thumb_dir: &Path,
    anchors: &mut [Option<(u64, u64)>],
) {
    const EMIT_EVERY: usize = 128;
    let total = indexes.len();
    let _ = app.emit("video-duplicate-progress", DuplicateProgress { processed: 0, total, stage: "anchor" });
    let mut fresh: Vec<(String, i64, i64, String)> = Vec::with_capacity(EMIT_EVERY);
    for (processed, &index) in indexes.iter().enumerate() {
        let (row, mtime) = &rows[index];
        let source = match row.thumbnail_path.as_deref() {
            Some(thumb) if Path::new(thumb).exists() => Some(thumb.to_string()),
            _ => {
                // 抽帧落到缩略图目录并登记：这趟活儿顺手把缺的封面补齐，
                // 下次「生成缩略图」就不必再为这条重复解码。
                let out = thumb_dir.join(format!("{}.jpg", row.id));
                match scanner::extract_thumbnail(&row.path, &out, row.duration) {
                    Ok(()) => {
                        if let Ok(conn) = app.state::<AppState>().db.lock() {
                            let _ = db::set_thumbnail(&conn, &row.id, &out.to_string_lossy());
                        }
                        Some(out.to_string_lossy().to_string())
                    }
                    Err(_) => None,
                }
            }
        };
        if let Some((phash, dhash)) = source.as_deref().and_then(scanner::image_frame_hashes) {
            anchors[index] = Some((phash, dhash));
            fresh.push((row.id.clone(), phash as i64, dhash as i64, mtime.clone()));
        }
        let done = processed + 1;
        if done % EMIT_EVERY == 0 || done == total {
            flush(app, &mut fresh, db::save_video_anchors);
            let _ = app.emit("video-duplicate-progress", DuplicateProgress { processed: done, total, stage: "anchor" });
        }
    }
    flush_with_retry(app, &mut fresh, db::save_video_anchors);
}

// flush_anchors / retry_flush / flush_frames 已删，改用 commands.rs 共享的 flush / flush_with_retry

/// 候选才走的第二趟：每个时间点一次 ffmpeg 快进定位（约 0.3s/处），
/// 所以这一趟的代价只落在锚点对得上的那几条上。
fn ensure_frames(
    app: &tauri::AppHandle,
    rows: &[(db::VideoSig, String)],
    indexes: &[usize],
    frames: &mut [Option<[(u64, u64); 2]>],
) {
    const EMIT_EVERY: usize = 16;
    let total = indexes.len();
    let _ = app.emit("video-duplicate-progress", DuplicateProgress { processed: 0, total, stage: "verify" });
    let mut fresh: Vec<(String, i64, i64, i64, i64, String)> = Vec::with_capacity(EMIT_EVERY);
    for (processed, &index) in indexes.iter().enumerate() {
        let (row, mtime) = &rows[index];
        let [mid, tail] = scanner::video_sample_times(row.duration);
        let picked = scanner::video_frame_hashes(&row.path, mid)
            .zip(scanner::video_frame_hashes(&row.path, tail));
        if let Some(pair) = picked {
            frames[index] = Some([pair.0, pair.1]);
            fresh.push((
                row.id.clone(),
                pair.0 .0 as i64,
                pair.0 .1 as i64,
                pair.1 .0 as i64,
                pair.1 .1 as i64,
                mtime.clone(),
            ));
        }
        let done = processed + 1;
        if done % EMIT_EVERY == 0 || done == total {
            flush(app, &mut fresh, db::save_video_frames);
            let _ = app.emit("video-duplicate-progress", DuplicateProgress { processed: done, total, stage: "verify" });
        }
    }
    flush_with_retry(app, &mut fresh, db::save_video_frames);
}

/// 找出画面相同的重复视频组：不再比文件字节（重压制、重封装过就漏判），
/// 改成三处采样点的画面指纹——锚点用已有缩略图（10% 处），中段和结尾
/// 各抽一帧（50%、85% 处）。三处都对得上才算同一部片子。
/// 每组按添加时间升序（第一个是要保留的原件），大的组排前面。
#[tauri::command]
pub async fn find_duplicate_videos(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Vec<String>>, String> {
    // 判据换成解码画面之后，ffmpeg 成了硬依赖（读文件字节不需要它）
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法比对视频画面。请安装 ffmpeg 并加入 PATH。".into());
    }
    // 磁盘上已经不存在的条目不参与判定；mtime 同时当指纹缓存的钥匙用
    let all_rows: Vec<db::VideoSig> = {
        let conn = state.db.lock().map_err_str()?;
        db::get_video_sigs(&conn).map_err_str()?
    };
    // 缓存的过期判断按"库里的记录数"，所以要在丢掉读不到 mtime 的那些之前数
    let library_count = all_rows.len();
    let thumb_dir = app.path().app_data_dir().map_err_str()?.join("thumbnails");

    let groups = tauri::async_runtime::spawn_blocking(move || {
        // 磁盘上已经不存在的条目不参与判定；mtime 同时当指纹缓存的钥匙用。
        // 逐文件 stat 挪进阻塞线程，别在 async 命令体里占住 tokio worker
        let rows: Vec<(db::VideoSig, String)> = all_rows
            .into_iter()
            .filter_map(|row| {
                let mtime = scanner::modified_stamp(Path::new(&row.path))?;
                Some((row, mtime))
            })
            .collect();
        let _ = std::fs::create_dir_all(&thumb_dir);
        let mut anchors: Vec<Option<(u64, u64)>> = Vec::with_capacity(rows.len());
        let mut frames: Vec<Option<[(u64, u64); 2]>> = Vec::with_capacity(rows.len());
        let mut need_anchor: Vec<usize> = Vec::new();
        for (index, (row, mtime)) in rows.iter().enumerate() {
            match row.cached_anchor(mtime) {
                Some(anchor) => {
                    anchors.push(Some(anchor));
                    frames.push(row.cached_frames(mtime));
                }
                None => {
                    anchors.push(None);
                    frames.push(None);
                    need_anchor.push(index);
                }
            }
        }
        ensure_anchors(&app, &rows, &need_anchor, &thumb_dir, anchors.as_mut_slice());

        let bucketed: Vec<usize> = (0..rows.len()).filter(|&i| anchors[i].is_some()).collect();
        let bucket_anchors = |slot: usize, other: usize| {
            matches!((anchors[bucketed[slot]], anchors[bucketed[other]]),
                (Some(a), Some(b)) if frames_match(a, b))
        };
        let candidates: Vec<usize> = cluster_by(bucketed.len(), bucket_anchors)
            .into_iter()
            .flatten()
            .map(|slot| bucketed[slot])
            .collect();

        let need_frames: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| frames[i].is_none())
            .collect();
        ensure_frames(&app, &rows, &need_frames, frames.as_mut_slice());

        let judged: Vec<FrameEntry> = candidates
            .iter()
            .filter_map(|&i| {
                let anchor = anchors[i]?;
                let [mid, tail] = frames[i]?;
                Some((rows[i].0.id.clone(), rows[i].0.created_at.clone(), [anchor, mid, tail]))
            })
            .collect();
        let groups = group_by_frames(&judged);
        // 这一趟热跑也要几分钟，落一份缓存，重启后直接接着看
        write_cache(
            &app,
            VIDEO_DUPLICATE_CACHE,
            &VideoDuplicateCache {
                library_count,
                groups: groups.clone(),
            },
        );
        groups
    })
    .await
    .map_err_str()?;

    Ok(groups)
}

/// 重复视频检测的落盘缓存：`<app_data>/video-duplicate-cache.json`。
/// 与图片侧同一个理由：跑一趟要等几分钟，重启后先把上一趟的组恢复出来看。
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDuplicateCache {
    pub library_count: usize,
    pub groups: Vec<Vec<String>>,
}

/// 上一趟的重复视频结果（没有则 null）。恢复出来的名单会照现在的库裁一遍再显示
#[tauri::command]
pub async fn get_video_duplicate_cache(
    app: tauri::AppHandle,
) -> Result<Option<VideoDuplicateCache>, String> {
    read_cache(&app, VIDEO_DUPLICATE_CACHE).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把最低位的 n 个 bit 翻掉，用来精确构造汉明距离
    fn flip(value: u64, n: u32) -> u64 {
        value ^ if n == 0 { 0 } else { (1u64 << n) - 1 }
    }

    #[test]
    fn test_frames_match_needs_both_hashes_close() {
        assert!(frames_match((0, 0), (flip(0, DUP_FRAME_DIST), flip(0, DUP_FRAME_DIST))));
        assert!(!frames_match((0, 0), (flip(0, DUP_FRAME_DIST + 1), 0)), "pHash 超线就不算同一画面");
        assert!(!frames_match((0, 0), (0, flip(0, DUP_FRAME_DIST + 1))), "dHash 超线就不算同一画面");
    }

    /// 锚点是从缩略图（480px JPEG）派生的，而复核帧直接取自源文件：
    /// 缩略图这一路必须也能认出"压制参数不同的同一部片子"，否则候选都圈不出来。
    #[test]
    fn test_anchor_from_thumbnail_survives_reencoding() {
        if !scanner::ffmpeg_available() {
            eprintln!("跳过：本机未安装 ffmpeg");
            return;
        }
        let dir = std::env::temp_dir().join(format!("viewman-anchor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let render = |name: &str, source: &str, extra: &[&str]| {
            let out = dir.join(name);
            let ok = std::process::Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", source, "-pix_fmt", "yuv420p"])
                .args(extra)
                .arg(&out)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "生成测试视频失败");
            out.to_string_lossy().to_string()
        };
        let pattern = "testsrc=size=640x480:rate=25:duration=6";
        let original = render("orig.mp4", pattern, &["-b:v", "2M"]);
        let reencoded = render("again.mp4", pattern, &["-vf", "scale=320:240", "-b:v", "250k"]);
        let other = render("other.mp4", "testsrc2=size=640x480:rate=25:duration=6", &["-b:v", "2M"]);

        let anchor = |video: &str, name: &str| {
            let thumb = dir.join(name);
            scanner::extract_thumbnail(video, &thumb, Some(6.0)).unwrap();
            scanner::image_frame_hashes(&thumb.to_string_lossy()).expect("缩略图该能取出锚点指纹")
        };
        let (a, b, c) = (anchor(&original, "a.jpg"), anchor(&reencoded, "b.jpg"), anchor(&other, "c.jpg"));
        assert!(frames_match(a, b), "重压制过的同一部片子，锚点指纹距离超出 {DUP_FRAME_DIST} 就会漏判");
        assert!(!frames_match(a, c), "另一部片子也对得上的话，候选桶就等于是没判据");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_group_by_frames_requires_all_three_slots() {
        let same = [(0u64, 0u64); 3];
        // 只有中段差一点：三处都得过线，所以这一条不配组
        let mid_off = [(0, 0), (flip(0, 20), 0), (0, 0)];
        let entries: Vec<FrameEntry> = vec![
            ("v1".into(), "1".into(), same),
            ("v2".into(), "2".into(), [(flip(0, 4), flip(0, 4)), same[1], same[2]]),
            ("v3".into(), "3".into(), mid_off),
            ("v4".into(), "4".into(), mid_off),
        ];
        assert_eq!(group_by_frames(&entries), vec![vec!["v1".to_string(), "v2".to_string()], vec!["v3".to_string(), "v4".to_string()]]);
    }

    #[test]
    fn test_group_by_frames_keeps_the_earliest_added_as_keeper() {
        let entries: Vec<FrameEntry> = vec![
            ("late".into(), "2024".into(), [(0, 0); 3]),
            ("early".into(), "2020".into(), [(0, 0); 3]),
        ];
        // 第一个是删除时保留的那条，必须是最早添加的
        assert_eq!(group_by_frames(&entries), vec![vec!["early".to_string(), "late".to_string()]]);
    }

    /// 候选桶是并查集圈出来的，允许链式相连；真正定组要逐条和保留原件比
    #[test]
    fn test_group_by_frames_does_not_inherit_chain_similarity() {
        let tail = |bits: u32| [(0u64, 0u64), (0, 0), (flip(0, bits), 0)];
        let entries: Vec<FrameEntry> = vec![
            ("a".into(), "1".into(), tail(0)),
            ("b".into(), "2".into(), tail(4)),
            ("c".into(), "3".into(), tail(8)),
        ];
        assert!(frames_match(entries[0].2[2], entries[1].2[2]), "a 和 b 该对得上");
        assert!(frames_match(entries[1].2[2], entries[2].2[2]), "b 和 c 该对得上");
        assert!(!frames_match(entries[0].2[2], entries[2].2[2]), "但 a 和 c 对不上");
        // c 只跟中间的 b 像，跟保留的 a 不像 → 不能靠 b 蹭进组
        assert_eq!(group_by_frames(&entries), vec![vec!["a".to_string(), "b".to_string()]]);
    }

    /// 候选桶按锚点的邻近关系并组：落单的不开桶，链式的允许并进来
    #[test]
    fn test_cluster_by_buckets_only_near_matches() {
        let sigs = [0u64, 0b1, 0b11, 0b1111_1111_1111];
        let clusters = cluster_by(sigs.len(), |i, j| {
            scanner::hamming_distance(sigs[i], sigs[j]) <= 3
        });
        assert_eq!(clusters, vec![vec![0, 1, 2]], "前三条并成一桶，第四条落单不成组");
    }
}
