use rusqlite::{Connection, OptionalExtension, Result, params};
use crate::models::{RecentlyPlayed, Video, VideoProgress, WatchProgress};

pub(crate) fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS videos (
            id TEXT PRIMARY KEY,
            path TEXT UNIQUE NOT NULL,
            filename TEXT NOT NULL,
            duration REAL,
            width INTEGER,
            height INTEGER,
            file_size INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            thumbnail_path TEXT
        );
        CREATE TABLE IF NOT EXISTS watch_progress (
            id TEXT PRIMARY KEY,
            video_id TEXT NOT NULL UNIQUE,
            position REAL NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (video_id) REFERENCES videos(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );"
    )
}

/// 读取字符串设置项
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0))
        .optional()
}

/// 写入（覆盖）字符串设置项
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![key, value],
    )?;
    Ok(())
}

/// 老库补列：CREATE TABLE IF NOT EXISTS 不会给已存在的表加字段，必须显式迁移
fn ensure_columns(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(videos)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?;
    if !columns.iter().any(|c| c == "thumbnail_path") {
        conn.execute("ALTER TABLE videos ADD COLUMN thumbnail_path TEXT", [])?;
    }
    Ok(())
}

pub fn init_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    create_tables(&conn)?;
    ensure_columns(&conn)?;
    Ok(conn)
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

pub fn insert_video(conn: &Connection, video: &Video) -> Result<()> {
    conn.execute(
        "INSERT INTO videos (id, path, filename, duration, width, height, file_size, created_at, thumbnail_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(path) DO UPDATE SET filename=excluded.filename, duration=COALESCE(excluded.duration,videos.duration),
         width=COALESCE(excluded.width,videos.width), height=COALESCE(excluded.height,videos.height), file_size=excluded.file_size,
         thumbnail_path=COALESCE(excluded.thumbnail_path,videos.thumbnail_path)",
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

pub fn upsert_progress(conn: &Connection, video_id: &str, position: f64) -> Result<()> {
    conn.execute(
        "INSERT INTO watch_progress (id, video_id, position, updated_at) VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(video_id) DO UPDATE SET position = ?3, updated_at = datetime('now')",
        params![uuid::Uuid::new_v4().to_string(), video_id, position],
    )?;
    Ok(())
}

pub fn get_progress(conn: &Connection, video_id: &str) -> Result<Option<WatchProgress>> {
    let mut stmt = conn.prepare(
        "SELECT id, video_id, position, updated_at FROM watch_progress WHERE video_id = ?1"
    )?;
    let mut rows = stmt.query_map(params![video_id], |row| {
        Ok(WatchProgress {
            id: row.get(0)?,
            video_id: row.get(1)?,
            position: row.get(2)?,
            updated_at: row.get(3)?,
        })
    })?;
    match rows.next() {
        Some(Ok(progress)) => Ok(Some(progress)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

pub fn get_recently_played(conn: &Connection, limit: i64) -> Result<Vec<RecentlyPlayed>> {
    let mut stmt = conn.prepare(
        "SELECT v.id, v.path, v.filename, v.duration, v.width, v.height, v.file_size, v.created_at, wp.position, wp.updated_at, v.thumbnail_path
         FROM watch_progress wp
         JOIN videos v ON v.id = wp.video_id
         ORDER BY wp.updated_at DESC
         LIMIT ?1"
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(RecentlyPlayed {
            video: Video {
                id: row.get(0)?,
                path: row.get(1)?,
                filename: row.get(2)?,
                duration: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                file_size: row.get(6)?,
                created_at: row.get(7)?,
                thumbnail_path: row.get(10)?,
            },
            position: row.get(8)?,
            updated_at: row.get(9)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(rows)
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

pub fn get_videos_with_progress(conn: &Connection) -> Result<Vec<VideoProgress>> {
    let mut stmt = conn.prepare(
        "SELECT v.id, v.path, v.filename, v.duration, v.width, v.height, v.file_size, v.created_at, wp.position, v.thumbnail_path
         FROM videos v LEFT JOIN watch_progress wp ON v.id = wp.video_id
         ORDER BY v.filename"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(VideoProgress {
            video: Video {
                id: row.get(0)?,
                path: row.get(1)?,
                filename: row.get(2)?,
                duration: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                file_size: row.get(6)?,
                created_at: row.get(7)?,
                thumbnail_path: row.get(9)?,
            },
            position: row.get::<_, Option<f64>>(8)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Video;

    fn sample_video(id: &str, path: &str) -> Video {
        Video {
            id: id.into(),
            path: path.into(),
            filename: "v.mp4".into(),
            duration: Some(60.0),
            width: Some(1280),
            height: Some(720),
            file_size: 100,
            created_at: "".into(),
            thumbnail_path: None,
        }
    }

    #[test]
    fn test_transaction_rollback_preserves_library() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("keep", "C:\\keep.mp4")).unwrap();

        // 在事务里删除旧条目并插入新条目，然后故意回滚
        let tx = conn.unchecked_transaction().unwrap();
        delete_videos_by_ids(&tx, &["keep".to_string()]).unwrap();
        insert_video(&tx, &sample_video("new", "C:\\new.mp4")).unwrap();
        tx.rollback().unwrap();

        assert_eq!(get_all_videos(&conn).unwrap().len(), 1);
        assert_eq!(get_video_path(&conn, "keep").unwrap().as_deref(), Some("C:\\keep.mp4"));
    }

    #[test]
    fn test_metadata_refresh_preserves_identity_and_progress() {
        let conn = setup_test_db();
        let old = sample_video("old", "C:/same.mp4");
        insert_video(&conn, &old).unwrap();
        upsert_progress(&conn, "old", 22.0).unwrap();
        let mut refreshed = sample_video("new-id", "C:/same.mp4");
        refreshed.duration = Some(150.0);
        insert_video(&conn, &refreshed).unwrap();
        let rows = get_all_videos(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "old");
        assert_eq!(rows[0].duration, Some(150.0));
        assert_eq!(get_progress(&conn, "old").unwrap().unwrap().position, 22.0);
    }

    #[test]
    fn test_failed_transaction_restores_deleted_progress() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("keep", "C:/keep.mp4")).unwrap();
        upsert_progress(&conn, "keep", 12.0).unwrap();
        insert_video(&conn, &sample_video("conflict", "C:/existing.mp4")).unwrap();
        {
            let tx = conn.unchecked_transaction().unwrap();
            delete_videos_by_ids(&tx, &["keep".into()]).unwrap();
            assert!(insert_video(&tx, &sample_video("conflict", "C:/other.mp4")).is_err());
        }
        assert!(get_video_path(&conn, "keep").unwrap().is_some());
        assert_eq!(get_progress(&conn, "keep").unwrap().unwrap().position, 12.0);
    }

    #[test]
    fn test_transaction_commit_applies_all() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("keep", "C:\\keep.mp4")).unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        delete_videos_by_ids(&tx, &["keep".to_string()]).unwrap();
        insert_video(&tx, &sample_video("new", "C:\\new.mp4")).unwrap();
        tx.commit().unwrap();

        let videos = get_all_videos(&conn).unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].id, "new");
    }

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_get_videos() {
        let conn = setup_test_db();
        let video = Video {
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
    fn test_progress_upsert() {
        let conn = setup_test_db();
        let video = Video {
            id: "v1".into(), path: "C:\\v.mp4".into(), filename: "v.mp4".into(),
            duration: None, width: None, height: None, file_size: 100, created_at: "".into(),
            thumbnail_path: None,
        };
        insert_video(&conn, &video).unwrap();

        upsert_progress(&conn, "v1", 30.5).unwrap();
        let p = get_progress(&conn, "v1").unwrap().unwrap();
        assert!((p.position - 30.5).abs() < 0.001);

        upsert_progress(&conn, "v1", 60.0).unwrap();
        let p = get_progress(&conn, "v1").unwrap().unwrap();
        assert!((p.position - 60.0).abs() < 0.001);
    }

    #[test]
    fn test_get_video_path() {
        let conn = setup_test_db();
        let video = Video {
            id: "v1".into(), path: "C:\\v.mp4".into(), filename: "v.mp4".into(),
            duration: None, width: None, height: None, file_size: 100, created_at: "".into(),
            thumbnail_path: None,
        };
        insert_video(&conn, &video).unwrap();

        assert_eq!(get_video_path(&conn, "v1").unwrap(), Some("C:\\v.mp4".to_string()));
        assert_eq!(get_video_path(&conn, "missing").unwrap(), None);
    }

    #[test]
    fn test_get_recently_played_returns_multiple() {
        let conn = setup_test_db();
        let v1 = Video {
            id: "a".into(), path: "C:\\a.mp4".into(), filename: "a.mp4".into(),
            duration: None, width: None, height: None, file_size: 100, created_at: "".into(),
            thumbnail_path: None,
        };
        let v2 = Video {
            id: "b".into(), path: "C:\\b.mp4".into(), filename: "b.mp4".into(),
            duration: None, width: None, height: None, file_size: 200, created_at: "".into(),
            thumbnail_path: None,
        };
        insert_video(&conn, &v1).unwrap();
        insert_video(&conn, &v2).unwrap();

        upsert_progress(&conn, "a", 10.0).unwrap();
        upsert_progress(&conn, "b", 20.0).unwrap();

        let result = get_recently_played(&conn, 30).unwrap();
        assert_eq!(result.len(), 2);
        let ids: Vec<&str> = result.iter().map(|r| r.video.id.as_str()).collect();
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
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

    #[test]
    fn test_settings_read_write() {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();

        // 未写入时为 None
        assert_eq!(get_setting(&conn, "scan_roots").unwrap(), None);

        set_setting(&conn, "scan_roots", r#"["D:\\视频"]"#).unwrap();
        assert_eq!(
            get_setting(&conn, "scan_roots").unwrap().as_deref(),
            Some(r#"["D:\\视频"]"#)
        );

        // 覆盖写入只保留最新值
        set_setting(&conn, "scan_roots", "[]").unwrap();
        assert_eq!(get_setting(&conn, "scan_roots").unwrap().as_deref(), Some("[]"));
    }
}
