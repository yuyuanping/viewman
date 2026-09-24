use rusqlite::{Connection, OptionalExtension, Result, params};

use crate::models::Image;

pub fn get_all_images(conn: &Connection) -> Result<Vec<Image>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, filename, width, height, file_size, created_at, thumbnail_path, modified_at FROM images ORDER BY filename"
    )?;
    let images = stmt.query_map([], |row| {
        Ok(Image {
            id: row.get(0)?,
            path: row.get(1)?,
            filename: row.get(2)?,
            width: row.get(3)?,
            height: row.get(4)?,
            file_size: row.get(5)?,
            created_at: row.get(6)?,
            thumbnail_path: row.get(7)?,
            modified_at: row.get(8)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(images)
}

/// 相似图检测要读的行。pHash 不进 Image 模型：19 万条清单不该为它多扛一列体积。
pub struct PhashRow {
    pub id: String,
    pub path: String,
    pub created_at: String,
    pub modified_at: Option<String>,
    /// 已缓存的 64 位指纹（存成有符号 INTEGER，位模式原样保留）
    pub phash: Option<i64>,
    /// 算这个指纹时文件的修改时间，跟当前对不上就说明图改过、缓存过期
    pub phash_modified_at: Option<String>,
}

impl PhashRow {
    /// 缓存能不能直接用
    pub fn cached_hash(&self) -> Option<u64> {
        match (self.phash, self.phash_modified_at.as_deref()) {
            (Some(hash), cached) if cached == self.modified_at.as_deref() => Some(hash as u64),
            _ => None,
        }
    }
}

pub fn get_phash_rows(conn: &Connection) -> Result<Vec<PhashRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, created_at, modified_at, phash, phash_modified_at FROM images ORDER BY created_at, id"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PhashRow {
            id: row.get(0)?,
            path: row.get(1)?,
            created_at: row.get(2)?,
            modified_at: row.get(3)?,
            phash: row.get(4)?,
            phash_modified_at: row.get(5)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

/// 一批指纹写回库（一次事务）：(image_id, 指纹, 指纹对应的文件修改时间)
pub fn save_image_phashes(conn: &Connection, updates: &[(String, i64, Option<String>)]) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "UPDATE images SET phash = ?1, phash_modified_at = ?2 WHERE id = ?3",
        )?;
        for (id, hash, modified_at) in updates {
            stmt.execute(rusqlite::params![hash, modified_at, id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn insert_image(conn: &Connection, image: &Image) -> Result<()> {
    conn.execute(
        "INSERT INTO images (id, path, filename, width, height, file_size, created_at, thumbnail_path, modified_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(path) DO UPDATE SET filename=excluded.filename, width=COALESCE(excluded.width,images.width),
         height=COALESCE(excluded.height,images.height), file_size=excluded.file_size,
         thumbnail_path=COALESCE(excluded.thumbnail_path,images.thumbnail_path),
         modified_at=COALESCE(excluded.modified_at,images.modified_at)",
        params![
            image.id, image.path, image.filename,
            image.width, image.height,
            image.file_size, image.created_at, image.thumbnail_path, image.modified_at
        ],
    )?;
    Ok(())
}

pub fn get_image_path(conn: &Connection, image_id: &str) -> Result<Option<String>> {
    conn.query_row("SELECT path FROM images WHERE id = ?1", params![image_id], |row| row.get(0))
        .optional()
}

pub fn update_image_path(conn: &Connection, image_id: &str, new_path: &str) -> Result<()> {
    conn.execute(
        "UPDATE images SET path = ?1 WHERE id = ?2",
        params![new_path, image_id],
    )?;
    Ok(())
}

pub fn set_image_thumbnail(conn: &Connection, image_id: &str, thumbnail_path: &str) -> Result<()> {
    conn.execute(
        "UPDATE images SET thumbnail_path = ?1 WHERE id = ?2",
        params![thumbnail_path, image_id],
    )?;
    Ok(())
}

/// 目标路径是否已被其他图片占用（Windows 路径大小写不敏感）
pub fn is_image_path_taken(conn: &Connection, path: &str, exclude_id: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM images WHERE LOWER(path) = LOWER(?1) AND id != ?2",
        params![path, exclude_id],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

pub fn delete_image(conn: &Connection, image_id: &str) -> Result<()> {
    conn.execute("DELETE FROM images WHERE id = ?1", params![image_id])?;
    Ok(())
}

pub fn delete_images_by_ids(conn: &Connection, ids: &[String]) -> Result<()> {
    for id in ids {
        delete_image(conn, id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{sample_image, setup_test_db};

    #[test]
    fn test_phash_cache_expires_when_file_mtime_changes() {
        let conn = setup_test_db();
        let mut row = sample_image("i1", "C:/pics/a.png");
        row.modified_at = Some("1700".to_string());
        insert_image(&conn, &row).unwrap();
        save_image_phashes(&conn, &[("i1".to_string(), -3_i64, Some("1700".to_string()))]).unwrap();

        let rows = get_phash_rows(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cached_hash(), Some(-3_i64 as u64));

        // 重新扫描把新 mtime 写回来（upsert 不动 phash 两列），指纹就该作废重算
        let mut edited = sample_image("i1", "C:/pics/a.png");
        edited.modified_at = Some("1800".to_string());
        insert_image(&conn, &edited).unwrap();
        let rows = get_phash_rows(&conn).unwrap();
        assert_eq!(rows[0].phash, Some(-3_i64));
        assert_eq!(rows[0].cached_hash(), None);
    }

    #[test]
    fn test_phash_cache_survives_rescan_of_unchanged_file() {
        let conn = setup_test_db();
        let mut row = sample_image("i1", "C:/pics/a.png");
        row.modified_at = Some("1700".to_string());
        insert_image(&conn, &row).unwrap();
        save_image_phashes(&conn, &[("i1".to_string(), 7_i64, Some("1700".to_string()))]).unwrap();

        // 同一份文件再扫一遍：mtime 没变，指纹仍然算数，不该又跑一次 ffmpeg
        insert_image(&conn, &row).unwrap();
        assert_eq!(get_phash_rows(&conn).unwrap()[0].cached_hash(), Some(7_u64));
    }

    #[test]
    fn test_insert_and_get_images() {
        let conn = setup_test_db();
        insert_image(&conn, &sample_image("i1", "C:\\pics\\a.png")).unwrap();
        let images = get_all_images(&conn).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].path, "C:\\pics\\a.png");
        assert_eq!(images[0].width, Some(800));
    }

    #[test]
    fn test_rescan_upsert_keeps_identity_and_thumbnail() {
        let conn = setup_test_db();
        insert_image(&conn, &sample_image("i1", "C:/pics/a.png")).unwrap();
        set_image_thumbnail(&conn, "i1", r"C:\cache\i1.jpg").unwrap();

        // build_image 总是发新 uuid，重新扫描同一路径不能换掉 id，也不能清掉已有封面
        insert_image(&conn, &sample_image("fresh-id", "C:/pics/a.png")).unwrap();
        let rows = get_all_images(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "i1");
        assert_eq!(rows[0].thumbnail_path.as_deref(), Some(r"C:\cache\i1.jpg"));
    }

    #[test]
    fn test_failed_scan_transaction_restores_library() {
        let conn = setup_test_db();
        insert_image(&conn, &sample_image("keep", "C:/pics/a.png")).unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        delete_images_by_ids(&tx, &["keep".to_string()]).unwrap();
        insert_image(&tx, &sample_image("new", "C:/pics/b.png")).unwrap();
        tx.rollback().unwrap();

        let rows = get_all_images(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "keep");
    }

    #[test]
    fn test_path_taken_ignores_case_and_self() {
        let conn = setup_test_db();
        insert_image(&conn, &sample_image("i1", "C:/pics/a.png")).unwrap();
        assert!(is_image_path_taken(&conn, "C:/PICS/A.PNG", "i2").unwrap());
        assert!(!is_image_path_taken(&conn, "C:/pics/a.png", "i1").unwrap());
        assert!(!is_image_path_taken(&conn, "C:/pics/b.png", "i1").unwrap());
    }

    #[test]
    fn test_image_and_video_tables_are_independent() {
        let conn = setup_test_db();
        insert_image(&conn, &sample_image("i1", "C:/mixed/a.jpg")).unwrap();
        crate::db::insert_video(&conn, &crate::db::sample_video("v1", "C:/mixed/a.jpg")).unwrap();

        assert_eq!(get_all_images(&conn).unwrap().len(), 1);
        assert_eq!(crate::db::get_all_videos(&conn).unwrap().len(), 1);

        delete_image(&conn, "i1").unwrap();
        assert!(get_all_images(&conn).unwrap().is_empty());
        assert_eq!(crate::db::get_all_videos(&conn).unwrap().len(), 1);
    }
}
