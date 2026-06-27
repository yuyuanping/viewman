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
