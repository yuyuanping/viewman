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
