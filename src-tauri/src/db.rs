use rusqlite::{Connection, Result};

mod images;
mod progress;
mod settings;
mod videos;

// 对 commands 保持平铺的函数路径（db::get_all_videos 等）
pub use images::*;
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
            thumbnail_path TEXT,
            video_codec TEXT,
            anchor_phash INTEGER,
            anchor_dhash INTEGER,
            mid_phash INTEGER,
            mid_dhash INTEGER,
            tail_phash INTEGER,
            tail_dhash INTEGER,
            sig_modified_at TEXT
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
        );
        CREATE TABLE IF NOT EXISTS images (
            id TEXT PRIMARY KEY,
            path TEXT UNIQUE NOT NULL,
            filename TEXT NOT NULL,
            width INTEGER,
            height INTEGER,
            file_size INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            thumbnail_path TEXT,
            modified_at TEXT,
            phash INTEGER,
            dhash INTEGER,
            sig_pixels BLOB,
            sig_modified_at TEXT
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
    if !columns.iter().any(|c| c == "video_codec") {
        conn.execute("ALTER TABLE videos ADD COLUMN video_codec TEXT", [])?;
    }
    // 重复判定用的画面指纹：锚点帧（缩略图那帧）+ 中段/结尾两处复核帧。
    // 复核帧只对"锚点对得上"的候选才抽，所以六列全允许为空。
    let has_video_column = |name: &str| columns.iter().any(|c| c == name);
    for col in [
        "anchor_phash",
        "anchor_dhash",
        "mid_phash",
        "mid_dhash",
        "tail_phash",
        "tail_dhash",
        "sig_modified_at",
    ] {
        if !has_video_column(col) {
            let kind = if col == "sig_modified_at" { "TEXT" } else { "INTEGER" };
            conn.execute(&format!("ALTER TABLE videos ADD COLUMN {col} {kind}"), [])?;
        }
    }
    drop(stmt);
    // 图片表可能尚未建（PRAGMA 对不存在的表返回空列集），有列且缺 modified_at 才补
    let mut stmt = conn.prepare("PRAGMA table_info(images)")?;
    let image_columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?;
    if !image_columns.is_empty() && !image_columns.iter().any(|c| c == "modified_at") {
        conn.execute("ALTER TABLE images ADD COLUMN modified_at TEXT", [])?;
    }
    // 相似图指纹缓存：全库 ffmpeg 是小时级的活儿，算过就要落库，别每次检测都从头跑
    let has_column = |name: &str| image_columns.iter().any(|c| c == name);
    if !image_columns.is_empty() && !has_column("phash") {
        conn.execute("ALTER TABLE images ADD COLUMN phash INTEGER", [])?;
    }
    // 时间戳列只管"这组图指纹"，早期版本只存 phash 一枚，名字跟着改了
    if !image_columns.is_empty() && !has_column("sig_modified_at") {
        if has_column("phash_modified_at") {
            conn.execute(
                "ALTER TABLE images RENAME COLUMN phash_modified_at TO sig_modified_at",
                [],
            )?;
        } else {
            conn.execute("ALTER TABLE images ADD COLUMN sig_modified_at TEXT", [])?;
        }
    }
    if !image_columns.is_empty() && !has_column("dhash") {
        conn.execute("ALTER TABLE images ADD COLUMN dhash INTEGER", [])?;
    }
    // 重复判定的最后一道看这 1KB 缩略像素：只有哈希撞车的候选才有，允许为空
    if !image_columns.is_empty() && !has_column("sig_pixels") {
        conn.execute("ALTER TABLE images ADD COLUMN sig_pixels BLOB", [])?;
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
pub(crate) fn sample_image(id: &str, path: &str) -> crate::models::Image {
    crate::models::Image {
        id: id.into(),
        path: path.into(),
        filename: "i.png".into(),
        width: Some(800),
        height: Some(600),
        file_size: 100,
        created_at: "".into(),
        thumbnail_path: None,
        modified_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_columns_migrates_the_legacy_signature_cache() {
        // 老库形状：有 phash + phash_modified_at（只有单路指纹的那个版本），没有 dhash
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn).unwrap();
        conn.execute("ALTER TABLE images RENAME COLUMN sig_modified_at TO phash_modified_at", []).unwrap();
        conn.execute("ALTER TABLE images DROP COLUMN dhash", []).unwrap();
        conn.execute(
            "INSERT INTO images (id, path, filename, file_size, modified_at, phash, phash_modified_at)
             VALUES ('i1', 'C:/pics/a.png', 'a.png', 10, '1700', 7, '1700')",
            [],
        )
        .unwrap();

        ensure_columns(&conn).unwrap();

        // 时间戳列沿用老数据（缓存不作废），dhash 补空列等下次检测补算第二路
        let row = get_image_sigs(&conn).unwrap().remove(0);
        assert_eq!(row.sig_modified_at.as_deref(), Some("1700"));
        assert_eq!(row.phash, Some(7));
        assert_eq!(row.dhash, None);
        assert_eq!(row.cached_sigs(), None);
        // 幂等：再跑一次不该报"列已存在"
        ensure_columns(&conn).unwrap();
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
}
