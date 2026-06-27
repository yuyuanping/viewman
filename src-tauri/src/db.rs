use rusqlite::{Connection, Result, params};
use crate::models::{Video, WatchProgress};

pub fn init_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS videos (
            id TEXT PRIMARY KEY,
            path TEXT UNIQUE NOT NULL,
            filename TEXT NOT NULL,
            duration REAL,
            width INTEGER,
            height INTEGER,
            file_size INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS watch_progress (
            id TEXT PRIMARY KEY,
            video_id TEXT NOT NULL UNIQUE,
            position REAL NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (video_id) REFERENCES videos(id) ON DELETE CASCADE
        );"
    )?;
    Ok(conn)
}

pub fn get_all_videos(conn: &Connection) -> Result<Vec<Video>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, filename, duration, width, height, file_size, created_at FROM videos ORDER BY filename"
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
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(videos)
}

pub fn insert_video(conn: &Connection, video: &Video) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO videos (id, path, filename, duration, width, height, file_size, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            video.id, video.path, video.filename,
            video.duration, video.width, video.height,
            video.file_size, video.created_at
        ],
    )?;
    Ok(())
}

pub fn get_existing_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM videos")?;
    let paths = stmt.query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>>>()?;
    Ok(paths)
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
        _ => Ok(None),
    }
}

pub fn get_videos_with_progress(conn: &Connection) -> Result<Vec<(Video, Option<f64>)>> {
    let mut stmt = conn.prepare(
        "SELECT v.id, v.path, v.filename, v.duration, v.width, v.height, v.file_size, v.created_at, wp.position
         FROM videos v LEFT JOIN watch_progress wp ON v.id = wp.video_id
         ORDER BY v.filename"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            Video {
                id: row.get(0)?,
                path: row.get(1)?,
                filename: row.get(2)?,
                duration: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                file_size: row.get(6)?,
                created_at: row.get(7)?,
            },
            row.get::<_, Option<f64>>(8)?,
        ))
    })?.collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Video;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS videos (
                id TEXT PRIMARY KEY, path TEXT UNIQUE NOT NULL, filename TEXT NOT NULL,
                duration REAL, width INTEGER, height INTEGER, file_size INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS watch_progress (
                id TEXT PRIMARY KEY, video_id TEXT NOT NULL UNIQUE, position REAL NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (video_id) REFERENCES videos(id) ON DELETE CASCADE
            );"
        ).unwrap();
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
        };
        insert_video(&conn, &video).unwrap();

        upsert_progress(&conn, "v1", 30.5).unwrap();
        let p = get_progress(&conn, "v1").unwrap().unwrap();
        assert!((p.position - 30.5).abs() < 0.001);

        upsert_progress(&conn, "v1", 60.0).unwrap();
        let p = get_progress(&conn, "v1").unwrap().unwrap();
        assert!((p.position - 60.0).abs() < 0.001);
    }
}
