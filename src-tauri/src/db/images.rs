use rusqlite::{Connection, OptionalExtension, Result, params};

use crate::models::Image;

fn image_from_row(row: &rusqlite::Row) -> Result<Image> {
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
}

const IMAGE_COLUMNS: &str =
    "id, path, filename, width, height, file_size, created_at, thumbnail_path, modified_at";

pub fn get_all_images(conn: &Connection) -> Result<Vec<Image>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, filename, width, height, file_size, created_at, thumbnail_path, modified_at FROM images ORDER BY filename"
    )?;
    let images = stmt.query_map([], image_from_row)?.collect::<Result<Vec<_>>>()?;
    Ok(images)
}

/// LIKE 通配符按 `ESCAPE '\'` 规则转义：目录和搜索词都是字面量，`%` `_` `\` 不能当通配符
fn like_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 目录过滤与前端 filterMedia 同一规则：统一 `\` 分隔、去结尾分隔符、
/// 按「目录\」前缀匹配（含全部子目录）。模式里所有反斜杠翻倍、结尾是裸 `%` 通配符
/// ——`\%` 会被 ESCAPE 解析成字面量百分号而不是通配符。
fn dir_like_pattern(dir: &str) -> String {
    let normalized = dir.replace('/', "\\");
    let trimmed = normalized.trim_end_matches('\\');
    format!("{}\\\\%", like_escape(trimmed))
}

/// 图片库视图：目录前缀 + 文件名子串过滤下推到 SQL，前端不再整表过桥。
/// 目录为 None/空 = 全库；搜索词为空串 = 不过滤。行序无所谓，排序由前端做
/// （SQLite 没有等价于 Intl.Collator 的中文拼音排序，硬排会改变现有语义）。
#[derive(serde::Serialize)]
pub struct ImageView {
    pub items: Vec<Image>,
    pub total: i64,
    pub total_size: i64,
}

pub fn get_image_view(conn: &Connection, dir: Option<&str>, search: &str) -> Result<ImageView> {
    let mut filters: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    if let Some(dir) = dir {
        if !dir.is_empty() {
            filters.push("path LIKE ? ESCAPE '\\'".into());
            values.push(dir_like_pattern(dir));
        }
    }
    let term = search.trim();
    if !term.is_empty() {
        filters.push("filename LIKE ? ESCAPE '\\'".into());
        values.push(format!("%{}%", like_escape(term)));
    }
    let where_clause = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };

    let sql = format!("SELECT {IMAGE_COLUMNS} FROM images {where_clause}");
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(values.iter()), image_from_row)?
        .collect::<Result<Vec<_>>>()?;

    let sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(file_size), 0) FROM images {where_clause}"
    );
    let (total, total_size) = conn.query_row(&sql, rusqlite::params_from_iter(values.iter()), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    Ok(ImageView { items, total, total_size })
}

/// 图片库统计：总数、缺封面数、每个文件父目录的直接文件数。
/// 前端用平表建目录树和算每根计数——几千个目录跟整表过桥是两个量级。
#[derive(serde::Serialize)]
pub struct ImageStats {
    pub total: i64,
    pub missing_thumbnails: i64,
    pub dirs: Vec<(String, i64)>,
}

pub fn get_image_stats(conn: &Connection) -> Result<ImageStats> {
    let mut stmt = conn.prepare("SELECT path, thumbnail_path IS NULL FROM images")?;
    let mut total = 0i64;
    let mut missing_thumbnails = 0i64;
    let mut dirs: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (path, no_thumbnail) = row?;
        total += 1;
        missing_thumbnails += no_thumbnail;
        // 没有分隔符的裸文件名进不了目录树，与前端 buildDirTree 的跳过规则一致
        if let Some((idx, _)) = path.char_indices().rfind(|&(_, c)| c == '\\' || c == '/') {
            let parent = path[..idx].replace('/', "\\");
            *dirs.entry(parent).or_insert(0) += 1;
        }
    }
    let mut dirs: Vec<(String, i64)> = dirs.into_iter().collect();
    dirs.sort();
    Ok(ImageStats { total, missing_thumbnails, dirs })
}

/// 缺封面的图片 id：封面批任务的待办清单，前端不再需要全量清单来 filter
pub fn get_missing_image_thumbnail_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM images WHERE thumbnail_path IS NULL")?;
    let ids = stmt.query_map([], |row| row.get(0))?.collect::<Result<Vec<_>>>()?;
    Ok(ids)
}

/// 按 id 批量取图。库不再整表下发后，检测面板的条目元数据（重复/相似/动图）
/// 和"这些 id 还活着吗"的收敛判定都走这里
pub fn get_images_by_ids(conn: &Connection, ids: &[String]) -> Result<Vec<Image>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT {IMAGE_COLUMNS} FROM images WHERE id IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let images = stmt
        .query_map(rusqlite::params_from_iter(ids.iter()), image_from_row)?
        .collect::<Result<Vec<_>>>()?;
    Ok(images)
}

/// 相似/重复检测要读的行。指纹不进 Image 模型：19 万条清单不该为它多扛几列体积。
pub struct ImageSig {
    pub id: String,
    pub path: String,
    pub created_at: String,
    pub modified_at: Option<String>,
    /// 已缓存的 64 位 pHash（存成有符号 INTEGER，位模式原样保留）
    pub phash: Option<i64>,
    /// 同一次解码算出的 64 位 dHash；老库里只有 phash 时它为空，下次检测补算
    pub dhash: Option<i64>,
    /// 32×32 灰度缩略像素（1024 字节）：重复判定的最后一道要看它。
    /// 只有"两枚哈希都相同"的候选才补算，所以大面积为空是正常的
    pub sig_pixels: Option<Vec<u8>>,
    /// 算这组指纹时文件的修改时间，跟当前对不上就说明图改过、缓存过期
    pub sig_modified_at: Option<String>,
}

impl ImageSig {
    /// 缓存的时间戳跟当前文件对得上，才说明这组指纹还是这张图现在的样子
    fn fresh(&self) -> bool {
        self.sig_modified_at.as_deref() == self.modified_at.as_deref()
    }

    /// 相似判定用的两枚感知哈希：缺任何一个都算没缓存（同趟解码出来的，不分开存）
    pub fn cached_sigs(&self) -> Option<(u64, u64)> {
        match (self.phash, self.dhash, self.fresh()) {
            (Some(p), Some(d), true) => Some((p as u64, d as u64)),
            _ => None,
        }
    }

    /// 重复判定用的缩略像素：前提是两枚哈希也在（否则连候选都进不了）
    pub fn cached_pixels(&self) -> Option<&[u8]> {
        match (self.cached_sigs(), self.sig_pixels.as_deref()) {
            (Some(_), Some(pixels)) if pixels.len() == 32 * 32 => Some(pixels),
            _ => None,
        }
    }
}

pub fn get_image_sigs(conn: &Connection) -> Result<Vec<ImageSig>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, created_at, modified_at, phash, dhash, sig_pixels, sig_modified_at FROM images ORDER BY created_at, id"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ImageSig {
            id: row.get(0)?,
            path: row.get(1)?,
            created_at: row.get(2)?,
            modified_at: row.get(3)?,
            phash: row.get(4)?,
            dhash: row.get(5)?,
            sig_pixels: row.get(6)?,
            sig_modified_at: row.get(7)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

/// 一批指纹写回库（一次事务）：(image_id, phash, dhash, 指纹对应的文件修改时间)。
/// 两枚出自同一次解码，所以总是一起写。
pub fn save_image_sigs(
    conn: &Connection,
    updates: &[(String, i64, i64, Option<String>)],
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "UPDATE images SET phash = ?1, dhash = ?2, sig_modified_at = ?3 WHERE id = ?4",
        )?;
        for (id, phash, dhash, modified_at) in updates {
            stmt.execute(rusqlite::params![phash, dhash, modified_at, id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// 一批候选的缩略像素写回库（一次事务）：(image_id, 32×32 灰度)。
/// 只给"两枚哈希相同"的候选算，所以量比指纹小两个数量级；写完下次检测就不必再解码。
pub fn save_image_pixels(conn: &Connection, updates: &[(String, Vec<u8>)]) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("UPDATE images SET sig_pixels = ?1 WHERE id = ?2")?;
        for (id, pixels) in updates {
            stmt.execute(rusqlite::params![pixels, id])?;
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
        save_image_sigs(&conn, &[("i1".to_string(), -3_i64, 11_i64, Some("1700".to_string()))]).unwrap();
        save_image_pixels(&conn, &[("i1".to_string(), vec![7u8; 32 * 32])]).unwrap();

        let rows = get_image_sigs(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cached_sigs(), Some((-3_i64 as u64, 11_u64)));
        assert_eq!(rows[0].cached_pixels(), Some(vec![7u8; 32 * 32].as_slice()));

        // 重新扫描把新 mtime 写回来（upsert 不动指纹三列），缓存就该作废重算
        let mut edited = sample_image("i1", "C:/pics/a.png");
        edited.modified_at = Some("1800".to_string());
        insert_image(&conn, &edited).unwrap();
        let rows = get_image_sigs(&conn).unwrap();
        assert_eq!(rows[0].phash, Some(-3_i64));
        assert_eq!(rows[0].cached_sigs(), None);
        // 哈希都作废了，候选都进不去，像素自然不算数
        assert_eq!(rows[0].cached_pixels(), None);

        // 半截像素（写坏/换了口径）不能当缓存用，否则比对拿到的两边长度都不一样
        conn.execute("UPDATE images SET sig_pixels = x'0102' WHERE id = 'i1'", []).unwrap();
        insert_image(&conn, &row).unwrap();
        assert_eq!(get_image_sigs(&conn).unwrap()[0].cached_pixels(), None);
    }

    #[test]
    fn test_phash_cache_survives_rescan_of_unchanged_file() {
        let conn = setup_test_db();
        let mut row = sample_image("i1", "C:/pics/a.png");
        row.modified_at = Some("1700".to_string());
        insert_image(&conn, &row).unwrap();
        save_image_sigs(&conn, &[("i1".to_string(), 7_i64, 9_i64, Some("1700".to_string()))]).unwrap();

        // 同一份文件再扫一遍：mtime 没变，指纹仍然算数，不该又跑一次 ffmpeg
        insert_image(&conn, &row).unwrap();
        assert_eq!(get_image_sigs(&conn).unwrap()[0].cached_sigs(), Some((7_u64, 9_u64)));
    }

    #[test]
    fn test_partial_cache_is_recomputed_when_dhash_is_missing() {
        let conn = setup_test_db();
        let mut row = sample_image("i1", "C:/pics/a.png");
        row.modified_at = Some("1700".to_string());
        insert_image(&conn, &row).unwrap();
        // 早期版本只存 phash：第二路没算过，必须当成没缓存，否则会拿单路结果当双路结论
        conn.execute(
            "UPDATE images SET phash = 7, sig_pixels = x'00', sig_modified_at = '1700' WHERE id = 'i1'",
            [],
        )
        .unwrap();
        assert_eq!(get_image_sigs(&conn).unwrap()[0].cached_sigs(), None);
        // 摘要也要跟着哈希一起认账：没双指纹的光有摘要进不了候选
        assert_eq!(get_image_sigs(&conn).unwrap()[0].cached_pixels(), None);
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

    fn insert_fixture(conn: &Connection) {
        // 目录大小写混着存的路径也要能被同一前缀命中（LIKE 对 ASCII 不区分大小写）
        for (id, path) in [
            ("i1", r"C:\pics\a.png"),
            ("i2", r"C:\pics\sub\b%percent.jpg"),
            ("i3", r"C:\pics\sub\c_my.jpg"),
            ("i4", r"C:\other\d.png"),
            ("i5", r"C:\PICS\e.png"),
        ] {
            let mut image = sample_image(id, path);
            image.filename = path.rsplit('\\').next().unwrap().into();
            image.file_size = 100;
            insert_image(conn, &image).unwrap();
        }
    }

    #[test]
    fn test_image_view_filters_by_dir_prefix_and_search() {
        let conn = setup_test_db();
        insert_fixture(&conn);

        // 目录前缀含子目录，与前端 filterMedia 的「目录\」前缀规则一致
        let view = get_image_view(&conn, Some(r"C:\pics"), "").unwrap();
        assert_eq!(view.total, 4);
        assert_eq!(view.total_size, 400);
        assert_eq!(view.items.len(), 4);

        // 搜索词只看文件名，% _ 是字面量不是通配符
        let view = get_image_view(&conn, Some(r"c:\pics"), "%").unwrap();
        assert_eq!(view.total, 1);
        assert_eq!(view.items[0].id, "i2");
        let view = get_image_view(&conn, None, "_my").unwrap();
        assert_eq!(view.items[0].id, "i3");

        // 全库视图：空目录与空搜索都不过滤
        let view = get_image_view(&conn, None, "").unwrap();
        assert_eq!(view.total, 5);
        assert_eq!(view.items.len(), 5);
    }

    #[test]
    fn test_image_stats_group_by_parent_dir() {
        let conn = setup_test_db();
        insert_fixture(&conn);

        let stats = get_image_stats(&conn).unwrap();
        assert_eq!(stats.total, 5);
        assert_eq!(stats.missing_thumbnails, 5);
        // 父目录按存储原样分键（大小写敏感），与旧目录树的建树规则一致
        let by_dir: std::collections::HashMap<&str, i64> =
            stats.dirs.iter().map(|(d, n)| (d.as_str(), *n)).collect();
        assert_eq!(by_dir.get(r"C:\pics").copied(), Some(1));
        assert_eq!(by_dir.get(r"C:\PICS").copied(), Some(1));
        assert_eq!(by_dir.get(r"C:\pics\sub").copied(), Some(2));
        assert_eq!(by_dir.get(r"C:\other").copied(), Some(1));

        set_image_thumbnail(&conn, "i1", r"C:\cache\i1.jpg").unwrap();
        let stats = get_image_stats(&conn).unwrap();
        assert_eq!(stats.missing_thumbnails, 4);
        assert_eq!(
            get_missing_image_thumbnail_ids(&conn).unwrap(),
            vec!["i2", "i3", "i4", "i5"]
        );
    }

    #[test]
    fn test_get_images_by_ids_returns_only_existing() {
        let conn = setup_test_db();
        insert_fixture(&conn);

        let rows = get_images_by_ids(&conn, &["i1".into(), "i3".into(), "gone".into()]).unwrap();
        let mut ids: Vec<&str> = rows.iter().map(|i| i.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["i1", "i3"]);
        assert!(get_images_by_ids(&conn, &[]).unwrap().is_empty());
    }
}
