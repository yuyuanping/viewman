use std::sync::Mutex;

mod players;
mod progress;
mod scan;
mod settings;
mod thumbnails;
mod videos;

// 对 lib.rs 的 invoke_handler 保持平铺的命令路径（commands::scan_directory 等）
pub use players::*;
pub use progress::*;
pub use scan::*;
pub use settings::*;
pub use thumbnails::*;
pub use videos::*;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
}
