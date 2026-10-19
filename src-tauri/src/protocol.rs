// 局域网 HTTPS 文件传输协议
use serde::{Deserialize, Serialize};

pub const VERSION: &str = "2";
pub const CHUNK_SIZE: u64 = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone)]
pub struct FileMeta {
    pub name: String,
    pub size: u64,
    /// 整文件 sha256（hex）
    pub sha256: String,
    pub chunk_size: u64,
    pub chunks: u64,
}

#[derive(Serialize, Deserialize)]
pub struct PrepareReq {
    pub sender_name: String,
    pub files: Vec<FileMeta>,
    pub total_bytes: u64,
}

#[derive(Serialize, Deserialize)]
pub struct PrepareResp {
    pub accepted: bool,
    pub token: String,
    #[serde(default)]
    pub message: String,
}

pub fn chunk_count(size: u64) -> u64 {
    if size == 0 {
        0
    } else {
        (size + CHUNK_SIZE - 1) / CHUNK_SIZE
    }
}
