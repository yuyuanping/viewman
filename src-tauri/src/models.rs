use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub path: String,
    pub filename: String,
    pub duration: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub file_size: i64,
    pub created_at: String,
    /// 缩略图缓存文件的绝对路径（由 ffmpeg 抽帧生成，缺失表示尚未生成）
    pub thumbnail_path: Option<String>,
}

/// 图片库条目：与视频分表存放，宽高来自 ffprobe，缺失表示尚未探测
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub id: String,
    pub path: String,
    pub filename: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub file_size: i64,
    pub created_at: String,
    /// 缩略图缓存文件的绝对路径（由 ffmpeg 缩放导出，缺失表示尚未生成）
    pub thumbnail_path: Option<String>,
    /// 文件修改时间（ISO8601 本地时间），扫描时从文件系统读取；照片整理排序用
    pub modified_at: Option<String>,
}

/// 一次扫描的增量结果：前端就地合并这两份数据，不必再整库重拉一次几十 MB
#[derive(Debug, Clone, Serialize)]
pub struct ScanOutcome<T> {
    /// 本轮新增或元数据刷新过的条目（已是落库后的最终值）
    pub items: Vec<T>,
    /// 本轮判定为"磁盘上已不存在"并从库里清掉的 id
    pub removed_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoProgress {
    pub video: Video,
    pub position: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentlyPlayed {
    pub video: Video,
    pub position: f64,
    pub updated_at: String,
}

/// "readable" 可正常打开 | "missing" 文件或记录不存在 | "unavailable" 被占用/无权限/离线 | "fake_image" 内容实为图片 | "error" 其他错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFileStatus {
    pub status: String,
    pub message: Option<String>,
}

/// 假视频转换为图片的结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversionResult {
    pub converted: usize,
    pub errors: Vec<String>,
}
