use rusqlite::{Connection, OptionalExtension, Result, params};

use crate::models::Video;

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
pub fn get_videos_with_max_duration(conn: &Connection, max: f64) -> Result<Vec<(String, String, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, duration FROM videos WHERE duration IS NOT NULL AND duration <= ?1",
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
    use crate::db::{ensure_columns, sample_video, setup_test_db};

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
    }
}
