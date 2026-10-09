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

/// LIKE 通配符按 `ESCAPE '\'` 规则转义：目录和搜索词都是字面量，`%` `_` `\` 不能当通配符
pub fn like_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}


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
            sig_modified_at TEXT,
            is_animated INTEGER
        );
        -- 全文索引：文件名/路径 FTS5，前缀搜索从 O(N) LIKE 到毫秒级 MATCH；
        -- content 同步由触发器维护，老库 ensure_fts 回填
        CREATE VIRTUAL TABLE IF NOT EXISTS videos_fts USING fts5(filename, path, content='videos', content_rowid='rowid');
        CREATE VIRTUAL TABLE IF NOT EXISTS images_fts USING fts5(filename, path, content='images', content_rowid='rowid');
        CREATE TRIGGER IF NOT EXISTS videos_fts_insert AFTER INSERT ON videos BEGIN
            INSERT INTO videos_fts(rowid, filename, path) VALUES (new.rowid, new.filename, new.path);
        END;
        CREATE TRIGGER IF NOT EXISTS videos_fts_delete AFTER DELETE ON videos BEGIN
            INSERT INTO videos_fts(videos_fts, rowid, filename, path) VALUES ('delete', old.rowid, old.filename, old.path);
        END;
        CREATE TRIGGER IF NOT EXISTS videos_fts_update AFTER UPDATE ON videos BEGIN
            INSERT INTO videos_fts(videos_fts, rowid, filename, path) VALUES ('delete', old.rowid, old.filename, old.path);
            INSERT INTO videos_fts(rowid, filename, path) VALUES (new.rowid, new.filename, new.path);
        END;
        CREATE TRIGGER IF NOT EXISTS images_fts_insert AFTER INSERT ON images BEGIN
            INSERT INTO images_fts(rowid, filename, path) VALUES (new.rowid, new.filename, new.path);
        END;
        CREATE TRIGGER IF NOT EXISTS images_fts_delete AFTER DELETE ON images BEGIN
            INSERT INTO images_fts(images_fts, rowid, filename, path) VALUES ('delete', old.rowid, old.filename, old.path);
        END;
        CREATE TRIGGER IF NOT EXISTS images_fts_update AFTER UPDATE ON images BEGIN
            INSERT INTO images_fts(images_fts, rowid, filename, path) VALUES ('delete', old.rowid, old.filename, old.path);
            INSERT INTO images_fts(rowid, filename, path) VALUES (new.rowid, new.filename, new.path);
        END;
        -- 十万级前缀 LIKE + 检测待办查询的索引：无索引即全表扫描
        CREATE INDEX IF NOT EXISTS idx_videos_path ON videos(path);
        CREATE INDEX IF NOT EXISTS idx_videos_filename ON videos(filename);
        CREATE INDEX IF NOT EXISTS idx_videos_codec ON videos(video_codec);
        CREATE INDEX IF NOT EXISTS idx_videos_thumbnail_null ON videos(thumbnail_path) WHERE thumbnail_path IS NULL;
        CREATE INDEX IF NOT EXISTS idx_images_path ON images(path);
        CREATE INDEX IF NOT EXISTS idx_images_filename ON images(filename);
        CREATE INDEX IF NOT EXISTS idx_images_phash_null ON images(phash) WHERE phash IS NULL;
        CREATE INDEX IF NOT EXISTS idx_images_animated_null ON images(is_animated) WHERE is_animated IS NULL;
        CREATE INDEX IF NOT EXISTS idx_images_thumbnail_null ON images(thumbnail_path) WHERE thumbnail_path IS NULL;
        CREATE INDEX IF NOT EXISTS idx_watch_progress_updated ON watch_progress(updated_at);"
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
    // 动图判定缓存：NULL = 没判定过（新入库 / 文件改动后作废），判过就沿用结论
    if !image_columns.is_empty() && !has_column("is_animated") {
        conn.execute("ALTER TABLE images ADD COLUMN is_animated INTEGER", [])?;
    }
    Ok(())
}

/// 老库 FTS 回填：触发器只管新写入，老记录需 rebuild 一次；幂等，可每次启动跑
pub(crate) fn ensure_fts(conn: &Connection) -> Result<()> {
    // 表不存在（极老库）则 create_tables 已建；这里只回填内容
    let videos: i64 = conn.query_row("SELECT COUNT(*) FROM videos_fts", [], |r| r.get(0)).unwrap_or(-1);
    if videos == 0 {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0)).unwrap_or(0);
        if n > 0 {
            conn.execute("INSERT INTO videos_fts(videos_fts) VALUES ('rebuild')", [])?;
        }
    }
    let images: i64 = conn.query_row("SELECT COUNT(*) FROM images_fts", [], |r| r.get(0)).unwrap_or(-1);
    if images == 0 {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0)).unwrap_or(0);
        if n > 0 {
            conn.execute("INSERT INTO images_fts(images_fts) VALUES ('rebuild')", [])?;
        }
    }
    Ok(())
}

/// 整库完整性校验：返回 "ok"，否则返回首条损坏描述。
/// 删除报 malformed 时先跑这个定性——是 FTS 索引坏（可无损重建）还是主表坏（需从备份恢复）。
pub fn integrity_check(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>>>()?;
    Ok(rows.into_iter().next().unwrap_or_else(|| "ok".into()))
}

/// 无损重建 FTS 索引：主表数据不动，只重建全文索引与回填。
/// 触发器仍在，重建后继续增量同步。返回重建后的 videos_fts / images_fts 行数。
pub fn rebuild_fts(conn: &Connection) -> Result<(i64, i64)> {
    // 表缺失（极老库中途升级失败）的先补上，IF NOT EXISTS 保证幂等
    create_tables(conn)?;
    conn.execute("INSERT INTO videos_fts(videos_fts) VALUES ('rebuild')", [])?;
    conn.execute("INSERT INTO images_fts(images_fts) VALUES ('rebuild')", [])?;
    let v: i64 = conn.query_row("SELECT COUNT(*) FROM videos_fts", [], |r| r.get(0))?;
    let i: i64 = conn.query_row("SELECT COUNT(*) FROM images_fts", [], |r| r.get(0))?;
    Ok((v, i))
}

/// 用户搜索词 → FTS5 前缀查询；含双引号/空词返回 None 调用方回退 LIKE。
/// 如 `宫崎` → `"宫崎"*`，多词按空格拆成 AND 前缀。
pub fn fts_query(term: &str) -> Option<String> {
    let t = term.trim();
    if t.is_empty() || t.contains('"') {
        return None;
    }
    let parts: Vec<String> = t
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.replace('*', "").replace('\'', "''")))
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(" AND "))
}

/// 只读查询的新连接：WAL 下多读并发，读命令不再跟写抢同一把全局锁。
/// 调用方每次用完即 drop；失败时调用方可回退到全局锁连接。
pub fn open_read_conn(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(conn)
}

pub fn init_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // WAL + NORMAL：封面批处理逐张 persist、扫描落大事务都是频繁写，WAL 显著提升
    // 写吞吐；NORMAL 下崩溃最多丢最后一个事务而不是损坏库——正好配合断点续跑
    // 主动制造"进程被杀"的场景，库本身必须经得起杀。
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // 单连接 + 多线程抢锁：扫描 8 线程逐条 insert 时极易 SQLITE_BUSY，
    // 5s busy_timeout 让写等待而不是直接失败（flush 重试是第二道保险）
    conn.pragma_update(None, "busy_timeout", 5000)?;
    create_tables(&conn)?;
    ensure_columns(&conn)?;
    // FTS 回填失败不致命（搜索回退 LIKE），记错即可
    let _ = ensure_fts(&conn);
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

    fn query_position(conn: &Connection, video_id: &str) -> Option<f64> {
        conn.query_row("SELECT position FROM watch_progress WHERE video_id = ?1", [video_id], |r| r.get(0)).ok()
    }

    #[test]
    fn test_fts_triggers_track_video_lifecycle() {
        let conn = setup_test_db();
        insert_video(&conn, &sample_video("v1", "C:/v/alpha.mp4")).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM videos_fts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);

        // MATCH 能命中文件名
        let hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM videos_fts WHERE videos_fts MATCH '\"alpha\"*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1);

        delete_videos_by_ids(&conn, &["v1".to_string()]).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM videos_fts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);

        // 回填幂等：空库 rebuild 不报错不增行
        ensure_fts(&conn).unwrap();
        ensure_fts(&conn).unwrap();
    }

    #[test]
    fn test_integrity_ok_and_fts_rebuild_is_lossless() {
        let conn = setup_test_db();
        assert_eq!(integrity_check(&conn).unwrap(), "ok");

        insert_video(&conn, &sample_video("v1", "C:/v/a.mp4")).unwrap();
        insert_image(&conn, &sample_image("i1", "C:/p/b.png")).unwrap();
        // 模拟 FTS 索引损坏（主表不动）：删掉索引行，MATCH 查不到
        conn.execute("DELETE FROM images_fts", []).unwrap();
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM images_fts WHERE images_fts MATCH '\"b\"*'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 0);

        let (v, i) = rebuild_fts(&conn).unwrap();
        assert_eq!((v, i), (1, 1));
        // 主表数据无损，索引恢复可用
        assert_eq!(get_all_videos(&conn).unwrap().len(), 1);
        assert_eq!(get_all_images(&conn).unwrap().len(), 1);
        assert_eq!(integrity_check(&conn).unwrap(), "ok");
    }

    #[test]
    fn test_init_db_enables_wal_on_file_db() {
        // 内存库测不出 journal_mode（永远返回 memory），必须走真实文件
        let path = std::env::temp_dir().join(format!("viewman-wal-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = init_db(path.to_str().unwrap()).unwrap();
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        drop(conn);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

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
        assert_eq!(query_position(&conn, "old"), Some(22.0));
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
        assert_eq!(query_position(&conn, "keep"), Some(12.0));
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
