use rusqlite::{Connection, OptionalExtension, Result, params};

use crate::models::Video;

/// 全部视频 id：孤儿缩略图清理时判断"这个缓存文件还有没有主"（视频与图片共用缓存目录）
pub fn get_all_video_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM videos")?;
    let ids = stmt.query_map([], |row| row.get(0))?.collect::<Result<Vec<_>>>()?;
    Ok(ids)
}

pub fn get_all_videos(conn: &Connection) -> Result<Vec<Video>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, filename, duration, width, height, file_size, created_at, thumbnail_path FROM videos ORDER BY filename"
    )?;
    let videos = stmt.query_map([], |row| {
        Ok(Video {
            id: row.get(0)?,
            path: row.get(1)?,
            filename: row.get(2)?,
            duration: row.get(3)?,
            width: row.get(4)?,
            height: row.get(5)?,
            file_size: row.get(6)?,
            created_at: row.get(7)?,
            thumbnail_path: row.get(8)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(videos)
}

/// 某个扫描根（含全部子目录）下的在库视频：扫描启动只需要本根的旧账，
/// 不必全表拉一遍——那会把其它命令堵在数据库锁后面。前缀已按
/// dir_prefix_lower 规范成小写带结尾反斜杠，LIKE 对 ASCII 不区分大小写。
fn like_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

pub fn get_videos_under_prefix(conn: &Connection, prefix_lower: &str) -> Result<Vec<Video>> {
    let pattern = format!("{}%", like_escape(prefix_lower));
    let mut stmt = conn.prepare(
        "SELECT id, path, filename, duration, width, height, file_size, created_at, thumbnail_path FROM videos WHERE path LIKE ?1 ESCAPE '\\'",
    )?;
    let videos = stmt.query_map(params![pattern], |row| {
        Ok(Video {
            id: row.get(0)?,
            path: row.get(1)?,
            filename: row.get(2)?,
            duration: row.get(3)?,
            width: row.get(4)?,
            height: row.get(5)?,
            file_size: row.get(6)?,
            created_at: row.get(7)?,
            thumbnail_path: row.get(8)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(videos)
}

/// 封面批次待办的最小行集 (id, 源路径, 时长, 封面路径)：续跑/生成前的逐条 stat
/// 要在锁外做，锁内只留这一下查询，全表九列的大行集没必要过一遍
pub fn get_video_thumbnail_entries(
    conn: &Connection,
) -> Result<Vec<(String, String, Option<f64>, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT id, path, duration, thumbnail_path FROM videos")?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn insert_video(conn: &Connection, video: &Video) -> Result<()> {
    conn.execute(
        "INSERT INTO videos (id, path, filename, duration, width, height, file_size, created_at, thumbnail_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(path) DO UPDATE SET filename=excluded.filename, duration=COALESCE(excluded.duration,videos.duration),
         width=COALESCE(excluded.width,videos.width), height=COALESCE(excluded.height,videos.height), file_size=excluded.file_size,
         thumbnail_path=COALESCE(excluded.thumbnail_path,videos.thumbnail_path),
         video_codec=CASE WHEN videos.file_size != excluded.file_size THEN NULL ELSE videos.video_codec END",
        params![
            video.id, video.path, video.filename,
            video.duration, video.width, video.height,
            video.file_size, video.created_at, video.thumbnail_path
        ],
    )?;
    Ok(())
}

pub fn set_thumbnail(conn: &Connection, video_id: &str, thumbnail_path: &str) -> Result<()> {
    conn.execute(
        "UPDATE videos SET thumbnail_path = ?1 WHERE id = ?2",
        params![thumbnail_path, video_id],
    )?;
    Ok(())
}

/// 重复视频检测要读的行。画面指纹不进 Video 模型：几千条清单不该为它多扛几列体积。
pub struct VideoSig {
    pub id: String,
    pub path: String,
    pub created_at: String,
    pub duration: Option<f64>,
    pub thumbnail_path: Option<String>,
    /// 锚点帧（缩略图那一帧）的双指纹，同一次解码出来，所以成对存
    pub anchor: Option<(i64, i64)>,
    /// 中段（50% 时长）复核帧的双指纹，只有锚点对得上的候选才会补算
    pub mid: Option<(i64, i64)>,
    /// 结尾（85% 时长）复核帧的双指纹，和中段同趟补算
    pub tail: Option<(i64, i64)>,
    /// 算这批指纹时文件的修改时间（videos 表没有 modified_at 列，所以拿实时 mtime 比）
    pub sig_modified_at: Option<String>,
}

impl VideoSig {
    fn fresh(&self, mtime: &str) -> bool {
        self.sig_modified_at.as_deref() == Some(mtime)
    }

    pub fn cached_anchor(&self, mtime: &str) -> Option<(u64, u64)> {
        match (self.anchor, self.fresh(mtime)) {
            (Some((p, d)), true) => Some((p as u64, d as u64)),
            _ => None,
        }
    }

    /// 两处复核帧：缺任何一处都算没验过（同批补算的，不拆开用）
    pub fn cached_frames(&self, mtime: &str) -> Option<[(u64, u64); 2]> {
        match (self.mid, self.tail, self.cached_anchor(mtime)) {
            (Some(mid), Some(tail), Some(_)) => {
                Some([(mid.0 as u64, mid.1 as u64), (tail.0 as u64, tail.1 as u64)])
            }
            _ => None,
        }
    }
}

fn pair(row: &rusqlite::Row<'_>, phash: usize, dhash: usize) -> Result<Option<(i64, i64)>> {
    let (p, d): (Option<i64>, Option<i64>) = (row.get(phash)?, row.get(dhash)?);
    Ok(match (p, d) {
        (Some(p), Some(d)) => Some((p, d)),
        _ => None,
    })
}

pub fn get_video_sigs(conn: &Connection) -> Result<Vec<VideoSig>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, created_at, duration, thumbnail_path,
                anchor_phash, anchor_dhash, mid_phash, mid_dhash, tail_phash, tail_dhash,
                sig_modified_at
         FROM videos ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(VideoSig {
            id: row.get(0)?,
            path: row.get(1)?,
            created_at: row.get(2)?,
            duration: row.get(3)?,
            thumbnail_path: row.get(4)?,
            anchor: pair(row, 5, 6)?,
            mid: pair(row, 7, 8)?,
            tail: pair(row, 9, 10)?,
            sig_modified_at: row.get(11)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

/// 写回锚点帧：(video_id, phash, dhash, 指纹对应的文件修改时间)。
/// 锚点一重算就把两处复核帧清空——它们是照着旧锚点验出来的，留着会把改过的文件判成新鲜。
pub fn save_video_anchors(
    conn: &Connection,
    updates: &[(String, i64, i64, String)],
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "UPDATE videos SET anchor_phash = ?1, anchor_dhash = ?2, sig_modified_at = ?3,
                mid_phash = NULL, mid_dhash = NULL, tail_phash = NULL, tail_dhash = NULL
             WHERE id = ?4",
        )?;
        for (id, phash, dhash, modified_at) in updates {
            stmt.execute(params![phash, dhash, modified_at, id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// 写回两处复核帧：(video_id, 中段 phash, 中段 dhash, 结尾 phash, 结尾 dhash, 指纹对应的修改时间)
pub fn save_video_frames(
    conn: &Connection,
    updates: &[(String, i64, i64, i64, i64, String)],
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "UPDATE videos SET mid_phash = ?1, mid_dhash = ?2, tail_phash = ?3, tail_dhash = ?4,
                sig_modified_at = ?5
             WHERE id = ?6",
        )?;
        for (id, mid_p, mid_d, tail_p, tail_d, modified_at) in updates {
            stmt.execute(params![mid_p, mid_d, tail_p, tail_d, modified_at, id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn get_video_path(conn: &Connection, video_id: &str) -> Result<Option<String>> {
    conn.query_row("SELECT path FROM videos WHERE id = ?1", params![video_id], |row| row.get(0))
        .optional()
}

/// 迁移库记录指向的新路径（文件本体由调用方先行移动）
pub fn update_video_path(conn: &Connection, video_id: &str, new_path: &str) -> Result<()> {
    conn.execute(
        "UPDATE videos SET path = ?1 WHERE id = ?2",
        params![new_path, video_id],
    )?;
    Ok(())
}

/// 永久转码后同步文件事实：路径（一般不变）与大小
pub fn update_video_location(conn: &Connection, video_id: &str, path: &str, file_size: i64) -> Result<()> {
    conn.execute(
        "UPDATE videos SET path = ?1, file_size = ?2 WHERE id = ?3",
        params![path, file_size, video_id],
    )?;
    Ok(())
}

/// 还没探测过编码的视频（id, path），供 HEVC 检测增量探测用
pub fn videos_without_codec(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, path FROM videos WHERE video_codec IS NULL ORDER BY filename")?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_video_codec(conn: &Connection, video_id: &str, codec: &str) -> Result<()> {
    conn.execute(
        "UPDATE videos SET video_codec = ?1 WHERE id = ?2",
        params![codec, video_id],
    )?;
    Ok(())
}

pub fn hevc_video_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM videos WHERE video_codec = 'hevc' ORDER BY filename")?;
    let ids = stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>>>()?;
    Ok(ids)
}

/// 路径是否已被 video_id 之外的记录占用（Windows 路径大小写不敏感）
pub fn is_path_taken(conn: &Connection, path: &str, exclude_id: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM videos WHERE LOWER(path) = LOWER(?1) AND id != ?2",
        params![path, exclude_id],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

pub fn get_video_path_and_duration(conn: &Connection, video_id: &str) -> Result<Option<(String, Option<f64>)>> {
    conn.query_row(
        "SELECT path, duration FROM videos WHERE id = ?1",
        params![video_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

/// (id, path, duration) of videos with a known duration not exceeding `max` seconds
/// 时长缺失（可能是图片伪装）或不超过 max 秒的候选，供静图检测使用
pub fn get_videos_with_max_duration(conn: &Connection, max: f64) -> Result<Vec<(String, String, Option<f64>)>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, duration FROM videos WHERE duration IS NULL OR duration <= ?1",
    )?;
    let rows = stmt.query_map(params![max], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect()
}

pub fn delete_video(conn: &Connection, video_id: &str) -> Result<()> {
    conn.execute("DELETE FROM watch_progress WHERE video_id = ?1", params![video_id])?;
    conn.execute("DELETE FROM videos WHERE id = ?1", params![video_id])?;
    Ok(())
}

pub fn delete_videos_by_ids(conn: &Connection, ids: &[String]) -> Result<()> {
    for id in ids {
        delete_video(conn, id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ensure_columns, insert_video, sample_video, setup_test_db};

    #[test]
    fn test_videos_under_prefix_scopes_to_one_root() {
        let conn = setup_test_db();
        for (id, path) in [
            ("v1", r"D:\vid\one.mp4"),
            ("v2", r"D:\vid\sub\two.mp4"),
            ("v3", r"D:\vids2\three.mp4"),
            ("v4", r"E:\other\four.mp4"),
        ] {
            insert_video(&conn, &sample_video(id, path)).unwrap();
        }

        // 前缀含子目录、大小写不敏感；D:\vid 不能误匹配 D:\vids2
        let rows = get_videos_under_prefix(&conn, r"d:\vid\").unwrap();
        let mut ids: Vec<&str> = rows.iter().map(|v| v.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["v1", "v2"]);
        assert!(get_videos_under_prefix(&conn, r"d:\vids\").unwrap().is_empty());
    }

    #[test]
    fn test_video_thumbnail_entries_carry_duration_and_path() {
        let conn = setup_test_db();
        let mut video = sample_video("v1", r"D:\vid\one.mp4");
        video.duration = Some(61.5);
        insert_video(&conn, &video).unwrap();
        set_thumbnail(&conn, "v1", r"C:\cache\v1.jpg").unwrap();

        let rows = get_video_thumbnail_entries(&conn).unwrap();
        assert_eq!(
            rows,
            vec![(
                "v1".to_string(),
                r"D:\vid\one.mp4".to_string(),
                Some(61.5),
                Some(r"C:\cache\v1.jpg".to_string()),
            )]
        );
    }

    #[test]
    fn test_insert_and_get_videos() {
        let conn = setup_test_db();
        let video = crate::models::Video {
            id: "test-id".into(),
            path: "C:\\videos\\test.mp4".into(),
            filename: "test.mp4".into(),
            duration: Some(120.5),
            width: Some(1920),
            height: Some(1080),
            file_size: 1024 * 1024 * 50,
            created_at: "2026-01-01T00:00:00".into(),
            thumbnail_path: None,
        };
        insert_video(&conn, &video).unwrap();
        let videos = get_all_videos(&conn).unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].filename, "test.mp4");
    }

    #[test]
    fn test_get_video_path() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("v1", "C:\\v.mp4")).unwrap();

        assert_eq!(get_video_path(&conn, "v1").unwrap(), Some("C:\\v.mp4".to_string()));
        assert_eq!(get_video_path(&conn, "missing").unwrap(), None);
    }

    #[test]
    fn test_codec_cache_persists_until_file_size_changes() {
        let conn = setup_test_db();
        let mut v = sample_video("v1", "C:/v.mp4");
        insert_video(&conn, &v).unwrap();
        insert_video(&conn, &sample_video("v2", "C:/v2.mp4")).unwrap();

        // 新入库的文件都待探测
        assert_eq!(videos_without_codec(&conn).unwrap().len(), 2);

        set_video_codec(&conn, "v1", "hevc").unwrap();
        set_video_codec(&conn, "v2", "h264").unwrap();
        assert_eq!(videos_without_codec(&conn).unwrap().len(), 0);
        assert_eq!(hevc_video_ids(&conn).unwrap(), vec!["v1".to_string()]);

        // 重新扫描、大小不变：编码结论保留；大小变了（文件被替换）则失效重探
        insert_video(&conn, &sample_video("v1", "C:/v.mp4")).unwrap();
        assert_eq!(hevc_video_ids(&conn).unwrap(), vec!["v1".to_string()]);
        v.file_size += 1024;
        insert_video(&conn, &v).unwrap();
        assert_eq!(videos_without_codec(&conn).unwrap().iter().filter(|(id, _)| id == "v1").count(), 1);
        assert_eq!(hevc_video_ids(&conn).unwrap().len(), 0);
    }

    #[test]
    fn test_set_thumbnail_is_readable_and_survives_rescan() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("v1", "C:/v.mp4")).unwrap();

        set_thumbnail(&conn, "v1", r"C:\cache\v1.jpg").unwrap();
        let rows = get_all_videos(&conn).unwrap();
        assert_eq!(rows[0].thumbnail_path.as_deref(), Some(r"C:\cache\v1.jpg"));

        // 重新扫描时 build_video 的 thumbnail_path 为 None，不能把已有封面清掉
        insert_video(&conn, &sample_video("v1", "C:/v.mp4")).unwrap();
        let rows = get_all_videos(&conn).unwrap();
        assert_eq!(rows[0].thumbnail_path.as_deref(), Some(r"C:\cache\v1.jpg"));

        // 但显式传入新路径时应更新
        let mut updated = sample_video("v1", "C:/v.mp4");
        updated.thumbnail_path = Some(r"C:\cache\v1-new.jpg".into());
        insert_video(&conn, &updated).unwrap();
        assert_eq!(
            get_all_videos(&conn).unwrap()[0].thumbnail_path.as_deref(),
            Some(r"C:\cache\v1-new.jpg")
        );
    }

    #[test]
    fn test_migration_adds_thumbnail_column_to_legacy_db() {
        // 模拟升级前建好的旧表（没有 thumbnail_path 列）
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE videos (
                id TEXT PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                filename TEXT NOT NULL,
                duration REAL,
                width INTEGER,
                height INTEGER,
                file_size INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        ).unwrap();

        ensure_columns(&conn).unwrap();
        ensure_columns(&conn).unwrap(); // 必须幂等，重复启动不能报错

        insert_video(&conn, &sample_video("legacy", "C:/legacy.mp4")).unwrap();
        let rows = get_all_videos(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].thumbnail_path, None);
        // 画面指纹七列也是这条路子补上来的，否则老库启动后一进重复检测就 SQL 报无此列
        assert_eq!(get_video_sigs(&conn).unwrap().len(), 1);
    }

    #[test]
    fn test_video_sig_cache_follows_the_file_mtime() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("v1", "C:/v/a.mp4")).unwrap();
        save_video_anchors(&conn, &[("v1".to_string(), 7_i64, 9_i64, "1700".to_string())]).unwrap();

        let row = get_video_sigs(&conn).unwrap().remove(0);
        assert_eq!(row.cached_anchor("1700"), Some((7_u64, 9_u64)));
        assert_eq!(row.cached_anchor("1800"), None, "mtime 变了就该重算锚点");
        // 还没复核过：只有锚点时两处复核帧都算没有
        assert_eq!(row.cached_frames("1700"), None);

        save_video_frames(&conn, &[("v1".to_string(), 1, 2, 3, 4, "1700".to_string())]).unwrap();
        let row = get_video_sigs(&conn).unwrap().remove(0);
        assert_eq!(row.cached_frames("1700"), Some([(1_u64, 2), (3, 4)]));
    }

    /// 锚点是复核的前提：锚点一重算（文件改过），旧复核帧必须一起作废
    #[test]
    fn test_recomputing_the_anchor_clears_the_verification_frames() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("v1", "C:/v/a.mp4")).unwrap();
        save_video_anchors(&conn, &[("v1".to_string(), 7, 9, "1700".to_string())]).unwrap();
        save_video_frames(&conn, &[("v1".to_string(), 1, 2, 3, 4, "1700".to_string())]).unwrap();
        assert!(get_video_sigs(&conn).unwrap()[0].cached_frames("1700").is_some());

        save_video_anchors(&conn, &[("v1".to_string(), 11, 12, "1800".to_string())]).unwrap();
        let row = get_video_sigs(&conn).unwrap().remove(0);
        assert_eq!(row.cached_anchor("1800"), Some((11_u64, 12)));
        assert_eq!(row.mid, None, "旧复核帧不能留着");
        assert_eq!(row.tail, None);
        assert_eq!(row.cached_frames("1800"), None);
    }
}
