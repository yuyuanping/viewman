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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchProgress {
    pub id: String,
    pub video_id: String,
    pub position: f64,
    pub updated_at: String,
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

/// "readable" 可正常打开 | "missing" 文件或记录不存在 | "unavailable" 被占用/无权限/离线 | "error" 其他错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFileStatus {
    pub status: String,
    pub message: Option<String>,
}
