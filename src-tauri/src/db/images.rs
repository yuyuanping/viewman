use std::collections::HashSet;
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

/// 某个扫描根（含全部子目录）下的在库图片：扫描启动只需要本根的旧账，
/// 不必全表 19 万行拉一遍——那会把其它命令堵在数据库锁后面等上几秒。
/// 前缀已按 dir_prefix_lower 规范成小写带结尾反斜杠，LIKE 对 ASCII 不区分大小写
pub fn get_images_under_prefix(conn: &Connection, prefix_lower: &str) -> Result<Vec<Image>> {
    let pattern = format!("{}%", like_escape(prefix_lower));
    let mut stmt = conn.prepare(
        "SELECT id, path, filename, width, height, file_size, created_at, thumbnail_path, modified_at FROM images WHERE path LIKE ?1 ESCAPE '\\'",
    )?;
    let images = stmt.query_map(params![pattern], image_from_row)?.collect::<Result<Vec<_>>>()?;
    Ok(images)
}

/// 封面批次待办的最小行集 (id, 源路径, 封面路径)：续跑/生成前的逐条 stat
/// 要在锁外做，锁内只留这一下查询，全表九列的大行集没必要过一遍
pub fn get_image_thumbnail_entries(conn: &Connection) -> Result<Vec<(String, String, Option<String>)>> {
    let mut stmt = conn.prepare("SELECT id, path, thumbnail_path FROM images")?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
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

/// 扫描范围子句：路径落在任一根内（含根自身）才算。根清单为空 = 不设限，
/// 库里所有文件都算（无根用户保持原有行为）。
fn scope_clause(roots: &[String]) -> (Option<String>, Vec<String>) {
    if roots.is_empty() {
        return (None, Vec::new());
    }
    let clause = vec!["path LIKE ? ESCAPE '\\'"; roots.len()].join(" OR ");
    let values: Vec<String> = roots.iter().map(|r| dir_like_pattern(r)).collect();
    (Some(format!("({})", clause)), values)
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

pub fn get_image_view(conn: &Connection, dir: Option<&str>, search: &str, roots: &[String]) -> Result<ImageView> {
    let mut filters: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    let (scope, mut scope_values) = scope_clause(roots);
    if let Some(clause) = scope {
        filters.push(clause);
        values.append(&mut scope_values);
    }
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
/// 只统计扫描范围内的文件：范围外（如移动到根外的落点）的记录保留但不可见。
#[derive(serde::Serialize)]
pub struct ImageStats {
    pub total: i64,
    pub missing_thumbnails: i64,
    pub dirs: Vec<(String, i64)>,
}

pub fn get_image_stats(conn: &Connection, roots: &[String]) -> Result<ImageStats> {
    let (scope, values) = scope_clause(roots);
    let where_clause = scope
        .map(|c| format!("WHERE {}", c))
        .unwrap_or_default();
    let sql = format!("SELECT path, thumbnail_path IS NULL FROM images {where_clause}");
    let mut stmt = conn.prepare(&sql)?;
    let mut total = 0i64;
    let mut missing_thumbnails = 0i64;
    let mut dirs: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
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

/// 全部图片 id：孤儿缩略图清理时判断"这个缓存文件还有没有主"
pub fn get_all_image_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM images")?;
    let ids = stmt.query_map([], |row| row.get(0))?.collect::<Result<Vec<_>>>()?;
    Ok(ids)
}

/// 一条 IN 最多带多少个 id：SQLite 变量上限 32766，宽容度调大后组员能到 3.7 万，
/// 一条 SQL 直接报"too many SQL variables"，前端静默吞掉后面板就全空了。500 远低于新老上限
const IDS_PER_QUERY: usize = 500;

/// 按 id 批量取图。库不再整表下发后，检测面板的条目元数据（重复/相似/动图）
/// 和"这些 id 还活着吗"的收敛判定都走这里。
/// 与视图同一套扫描范围口径：移动到根外的落点后记录虽在，面板也当它已退库，
/// 否则相似列表里会一直挂着网格里已经看不见的图。
/// id 多于一条语句装得下时分片查（id 是主键，分片不影响结果）。
pub fn get_images_by_ids(conn: &Connection, ids: &[String], roots: &[String]) -> Result<Vec<Image>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let mut seen = HashSet::with_capacity(ids.len());
    let ids: Vec<&String> = ids.iter().filter(|id| seen.insert(id.as_str())).collect();
    let mut images = Vec::new();
    for chunk in ids.chunks(IDS_PER_QUERY) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let (scope, mut values) = scope_clause(roots);
        let mut filters: Vec<String> = Vec::new();
        if let Some(clause) = scope {
            filters.push(clause);
        }
        filters.push(format!("id IN ({placeholders})"));
        values.extend(chunk.iter().map(|id| (*id).clone()));
        let sql = format!("SELECT {IMAGE_COLUMNS} FROM images WHERE {}", filters.join(" AND "));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(values.iter()), image_from_row)?
            .collect::<Result<Vec<_>>>()?;
        images.extend(rows);
    }
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

fn get_image_sigs_where(conn: &Connection, where_clause: &str, values: &[String]) -> Result<Vec<ImageSig>> {
    let sql = format!(
        "SELECT id, path, created_at, modified_at, phash, dhash, sig_pixels, sig_modified_at FROM images {where_clause} ORDER BY created_at, id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
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

pub fn get_image_sigs(conn: &Connection) -> Result<Vec<ImageSig>> {
    get_image_sigs_where(conn, "", &[])
}

/// 扫描范围内的双指纹行：相似检测只比对落在扫描根内的图，
/// 范围外（如被移出根的落点）的记录保留但不可见，与图片库视图同一口径
pub fn get_image_sigs_scoped(conn: &Connection, roots: &[String]) -> Result<Vec<ImageSig>> {
    let (scope, values) = scope_clause(roots);
    let where_clause = scope
        .map(|c| format!("WHERE {}", c))
        .unwrap_or_default();
    get_image_sigs_where(conn, &where_clause, &values)
}

/// 扫描范围内的图片 id 集合：恢复旧检测结果缓存时，把范围外的组员按当前口径裁掉
pub fn image_ids_in_scope(conn: &Connection, roots: &[String]) -> Result<Vec<String>> {
    let (scope, values) = scope_clause(roots);
    let where_clause = scope
        .map(|c| format!("WHERE {}", c))
        .unwrap_or_default();
    let sql = format!("SELECT id FROM images {where_clause}");
    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt
        .query_map(rusqlite::params_from_iter(values.iter()), |row| row.get(0))?
        .collect::<Result<Vec<_>>>()?;
    Ok(ids)
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

/// 动图判定待办：is_animated 为 NULL 的行（新入库、文件改动后作废）。
/// 判定只读文件头很便宜，但几十万张全扫也要逐个开文件，判过就得落库
pub fn get_animated_flag_todo(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT id, path FROM images WHERE is_animated IS NULL")?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

/// 已判定的动图 id：命中清单直接出自库，不再重复解析
pub fn get_animated_image_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM images WHERE is_animated = 1")?;
    let ids = stmt.query_map([], |row| row.get(0))?.collect::<Result<Vec<_>>>()?;
    Ok(ids)
}

/// 一批判定结果写回（一次事务）：(image_id, 是否动图)
pub fn save_animated_flags(conn: &Connection, updates: &[(String, bool)]) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("UPDATE images SET is_animated = ?1 WHERE id = ?2")?;
        for (id, animated) in updates {
            stmt.execute(rusqlite::params![animated, id])?;
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
         modified_at=COALESCE(excluded.modified_at,images.modified_at),
         is_animated=CASE
            WHEN excluded.modified_at IS NOT NULL
                 AND images.modified_at IS NOT excluded.modified_at THEN NULL
            ELSE images.is_animated END",
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

/// 还没有指纹的在库图片 (id, 路径)：重复检测里"解不出画面"的就是这批，
/// 扩展名修正也只对它们有意义——解得出的图 ffmpeg 按内容探测兜着，错标也不碍事
pub fn unhashed_image_paths(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT id, path FROM images WHERE phash IS NULL")?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
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
    fn test_animated_flag_cache_is_keyed_on_mtime() {
        let conn = setup_test_db();
        let mut row = sample_image("i1", "C:/pics/a.gif");
        row.modified_at = Some("1700".to_string());
        insert_image(&conn, &row).unwrap();
        // 没判定过：进待办，不在命中清单
        assert_eq!(
            get_animated_flag_todo(&conn).unwrap(),
            vec![("i1".to_string(), "C:/pics/a.gif".to_string())]
        );
        assert!(get_animated_image_ids(&conn).unwrap().is_empty());

        save_animated_flags(&conn, &[("i1".to_string(), true)]).unwrap();
        assert!(get_animated_flag_todo(&conn).unwrap().is_empty());
        assert_eq!(get_animated_image_ids(&conn).unwrap(), vec!["i1".to_string()]);

        // 同一份文件再扫一遍：mtime 没变，判定结论沿用，不用再开文件
        insert_image(&conn, &row).unwrap();
        assert!(get_animated_flag_todo(&conn).unwrap().is_empty());
        assert_eq!(get_animated_image_ids(&conn).unwrap(), vec!["i1".to_string()]);

        // 文件被替换（mtime 变了）：旧结论作废重新进待办，命中清单也不再算它
        let mut edited = sample_image("i1", "C:/pics/a.gif");
        edited.modified_at = Some("1800".to_string());
        insert_image(&conn, &edited).unwrap();
        assert_eq!(get_animated_flag_todo(&conn).unwrap().len(), 1);
        assert!(get_animated_image_ids(&conn).unwrap().is_empty());

        save_animated_flags(&conn, &[("i1".to_string(), false)]).unwrap();
        assert!(get_animated_flag_todo(&conn).unwrap().is_empty());
        // 判成静图的不在命中清单里，但也不再是待办
        assert!(get_animated_image_ids(&conn).unwrap().is_empty());
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
    fn test_images_under_prefix_scopes_to_one_root() {
        let conn = setup_test_db();
        insert_fixture(&conn);

        // 前缀含子目录、大小写不敏感，与扫描时的 dir_prefix_lower 同一口径
        let rows = get_images_under_prefix(&conn, r"c:\pics\").unwrap();
        let mut ids: Vec<&str> = rows.iter().map(|i| i.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["i1", "i2", "i3", "i5"]);

        // 前缀边界：C:\pic 不能匹配 C:\pics（结尾分隔符挡着）
        assert!(get_images_under_prefix(&conn, r"c:\pic\").unwrap().is_empty());

        // 路径里的 % _ 是字面量，照常命中
        let rows = get_images_under_prefix(&conn, r"c:\pics\sub\").unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_thumbnail_entries_are_the_minimal_three_columns() {
        let conn = setup_test_db();
        insert_fixture(&conn);
        set_image_thumbnail(&conn, "i1", r"C:\cache\i1.jpg").unwrap();

        let rows = get_image_thumbnail_entries(&conn).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows.iter().find(|(id, _, _)| id == "i1").unwrap(),
            &("i1".to_string(), r"C:\pics\a.png".to_string(), Some(r"C:\cache\i1.jpg".to_string()))
        );
    }

    #[test]
    fn test_image_view_filters_by_dir_prefix_and_search() {
        let conn = setup_test_db();
        insert_fixture(&conn);

        // 目录前缀含子目录，与前端 filterMedia 的「目录\」前缀规则一致
        let view = get_image_view(&conn, Some(r"C:\pics"), "", &[]).unwrap();
        assert_eq!(view.total, 4);
        assert_eq!(view.total_size, 400);
        assert_eq!(view.items.len(), 4);

        // 搜索词只看文件名，% _ 是字面量不是通配符
        let view = get_image_view(&conn, Some(r"c:\pics"), "%", &[]).unwrap();
        assert_eq!(view.total, 1);
        assert_eq!(view.items[0].id, "i2");
        let view = get_image_view(&conn, None, "_my", &[]).unwrap();
        assert_eq!(view.items[0].id, "i3");

        // 全库视图：空目录与空搜索都不过滤
        let view = get_image_view(&conn, None, "", &[]).unwrap();
        assert_eq!(view.total, 5);
        assert_eq!(view.items.len(), 5);
    }

    #[test]
    fn test_image_stats_group_by_parent_dir() {
        let conn = setup_test_db();
        insert_fixture(&conn);

        let stats = get_image_stats(&conn, &[]).unwrap();
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
        let stats = get_image_stats(&conn, &[]).unwrap();
        assert_eq!(stats.missing_thumbnails, 4);
        assert_eq!(
            get_missing_image_thumbnail_ids(&conn).unwrap(),
            vec!["i2", "i3", "i4", "i5"]
        );
    }

    #[test]
    fn test_scope_roots_limit_view_and_stats() {
        let conn = setup_test_db();
        insert_fixture(&conn);

        // 范围 = C:\pics（含子目录）；C:\other 下的文件被隐藏但记录还在
        let roots = vec![r"C:\pics".to_string()];
        let view = get_image_view(&conn, None, "", &roots).unwrap();
        assert_eq!(view.total, 4);
        assert_eq!(view.items.len(), 4);
        let stats = get_image_stats(&conn, &roots).unwrap();
        assert_eq!(stats.total, 4);
        // LIKE 对 ASCII 大小写不敏感，C:\PICS 也在范围内
        assert!(stats.dirs.iter().all(|(d, _)| d.to_lowercase().starts_with("c:\\pics")));

        // 范围外目录即使显式查询也是空：它已经不在可见库里
        let view = get_image_view(&conn, Some(r"C:\other"), "", &roots).unwrap();
        assert_eq!(view.total, 0);

        // 根清单为空 = 不设限（无根用户保持原有行为）
        let view = get_image_view(&conn, None, "", &[]).unwrap();
        assert_eq!(view.total, 5);
    }

    #[test]
    fn test_get_images_by_ids_returns_only_existing() {
        let conn = setup_test_db();
        insert_fixture(&conn);

        let rows = get_images_by_ids(&conn, &["i1".into(), "i3".into(), "gone".into()], &[]).unwrap();
        let mut ids: Vec<&str> = rows.iter().map(|i| i.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["i1", "i3"]);
        assert!(get_images_by_ids(&conn, &[], &[]).unwrap().is_empty());

        // 扫描范围外（如移动到根外的落点）的记录按视图同一口径隐藏，面板据此裁组
        let roots = vec![r"C:\pics".to_string()];
        let rows = get_images_by_ids(&conn, &["i1".into(), "i4".into()], &roots).unwrap();
        assert_eq!(rows.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec!["i1"]);
    }

    #[test]
    fn test_get_images_by_ids_chunks_past_variable_limit() {
        let conn = setup_test_db();
        // 一次 IN 最多 500 个 id，给 1200 张：分片查回来的必须一张不少、也不多
        let all: Vec<String> = (0..1200).map(|n| format!("bulk{n}")).collect();
        for (n, id) in all.iter().enumerate() {
            let mut image = sample_image(id, &format!(r"C:\pics\{n}.png"));
            image.filename = format!("{n}.png");
            image.file_size = 100;
            insert_image(&conn, &image).unwrap();
        }
        let rows = get_images_by_ids(&conn, &all, &[]).unwrap();
        assert_eq!(rows.len(), 1200);

        // 重复 id 只回一行：面板拿它建 Map，重复行会被后到的覆盖掉，无害但没必要
        let mut with_dup = all[..3].to_vec();
        with_dup.extend_from_slice(&all[..3]);
        let rows = get_images_by_ids(&conn, &with_dup, &[]).unwrap();
        assert_eq!(rows.len(), 3);
    }
}
