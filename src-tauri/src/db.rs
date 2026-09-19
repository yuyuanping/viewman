use rusqlite::{Connection, Result};

mod progress;
mod settings;
mod videos;

// 对 commands 保持平铺的函数路径（db::get_all_videos 等）
pub use progress::*;
pub use settings::*;
pub use videos::*;

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

/// 老库补列：CREATE TABLE IF NOT EXISTS 不会给已存在的表加字段，必须显式迁移
pub(crate) fn ensure_columns(conn: &Connection) -> Result<()> {
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

#[cfg(test)]
pub(crate) fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    create_tables(&conn).unwrap();
    conn
}

#[cfg(test)]
pub(crate) fn sample_video(id: &str, path: &str) -> crate::models::Video {
    crate::models::Video {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
