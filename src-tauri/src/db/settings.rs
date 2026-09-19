use rusqlite::{Connection, OptionalExtension, Result, params};

/// 读取字符串设置项
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0))
        .optional()
}

/// 写入（覆盖）字符串设置项
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::setup_test_db;

    #[test]
    fn test_settings_read_write() {
        let conn = setup_test_db();

        // 未写入时为 None
        assert_eq!(get_setting(&conn, "scan_roots").unwrap(), None);

        set_setting(&conn, "scan_roots", r#"["D:\\视频"]"#).unwrap();
        assert_eq!(
            get_setting(&conn, "scan_roots").unwrap().as_deref(),
            Some(r#"["D:\\视频"]"#)
        );

        // 覆盖写入只保留最新值
        set_setting(&conn, "scan_roots", "[]").unwrap();
        assert_eq!(get_setting(&conn, "scan_roots").unwrap().as_deref(), Some("[]"));
    }
}
