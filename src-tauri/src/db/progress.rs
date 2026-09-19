use rusqlite::{Connection, Result, params};

use crate::models::{RecentlyPlayed, Video, VideoProgress, WatchProgress};

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
    use crate::db::{insert_video, sample_video, setup_test_db};

    #[test]
    fn test_progress_upsert() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("v1", "C:\\v.mp4")).unwrap();

        upsert_progress(&conn, "v1", 30.5).unwrap();
        let p = get_progress(&conn, "v1").unwrap().unwrap();
        assert!((p.position - 30.5).abs() < 0.001);

        upsert_progress(&conn, "v1", 60.0).unwrap();
        let p = get_progress(&conn, "v1").unwrap().unwrap();
        assert!((p.position - 60.0).abs() < 0.001);
    }

    #[test]
    fn test_get_recently_played_returns_multiple() {
        let conn = setup_test_db();
        let v1 = sample_video("a", "C:\\a.mp4");
        let mut v2 = sample_video("b", "C:\\b.mp4");
        v2.file_size = 200;
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
}
