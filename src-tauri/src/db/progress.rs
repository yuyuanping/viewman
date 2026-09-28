use rusqlite::{Connection, Result, params};

use super::videos::video_from_row;
use crate::models::{RecentlyPlayed, VideoProgress};

pub fn upsert_progress(conn: &Connection, video_id: &str, position: f64) -> Result<()> {
    conn.execute(
        "INSERT INTO watch_progress (id, video_id, position, updated_at) VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(video_id) DO UPDATE SET position = ?3, updated_at = datetime('now')",
        params![uuid::Uuid::new_v4().to_string(), video_id, position],
    )?;
    Ok(())
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
            video: video_from_row(row, 10)?,
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
            video: video_from_row(row, 9)?,
            position: row.get::<_, Option<f64>>(8)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{insert_video, sample_video, setup_test_db};

    fn query_position(conn: &Connection, video_id: &str) -> Option<f64> {
        conn.query_row("SELECT position FROM watch_progress WHERE video_id = ?1", [video_id], |r| r.get(0)).ok()
    }

    #[test]
    fn test_progress_upsert() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("v1", "C:\\v.mp4")).unwrap();

        upsert_progress(&conn, "v1", 30.5).unwrap();
        assert!((query_position(&conn, "v1").unwrap() - 30.5).abs() < 0.001);

        upsert_progress(&conn, "v1", 60.0).unwrap();
        assert!((query_position(&conn, "v1").unwrap() - 60.0).abs() < 0.001);
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
