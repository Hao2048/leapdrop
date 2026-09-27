// 传输协议共享类型（三通道复用的应用层消息）
use serde::{Deserialize, Serialize};

pub const VERSION: &str = "1";
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
    pub files: Vec<FileMeta>,
    pub total_bytes: u64,
}

/// fileIdx -> 已有分块序号（断点续传）
pub type ResumeMap = std::collections::HashMap<usize, Vec<u64>>;

#[derive(Serialize, Deserialize)]
pub struct PrepareResp {
    pub accepted: bool,
    pub token: String,
    #[serde(default)]
    pub resume: ResumeMap,
    #[serde(default)]
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct HelloResp {
    pub name: String,
    pub fp: String,
    pub id: String,
    pub v: String,
}

/// 配对请求（签名覆盖整个请求体原文）
#[derive(Serialize, Deserialize, Clone)]
pub struct PairReq {
    pub device_id: String,
    pub name: String,
    /// Ed25519 公钥（32 字节，base64）
    pub pk: String,
}

#[derive(Serialize, Deserialize)]
pub struct PairResp {
    pub accepted: bool,
    pub status: String, // "ok" | "already"
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub pk: String,
}

/// 403 需要配对的响应体
#[derive(Serialize, Deserialize)]
pub struct NeedPairing {
    pub error: String, // "need_pairing"
    pub device_id: String,
    pub name: String,
    pub pk: String,
}

/// 已配对设备（trusted.json 单条记录）
#[derive(Serialize, Deserialize, Clone)]
pub struct TrustedPeer {
    pub id: String,
    pub name: String,
    /// base64(32 字节 Ed25519 公钥)
    pub pk: String,
    pub paired_at: u64,
}

pub fn chunk_count(size: u64) -> u64 {
    if size == 0 {
        0
    } else {
        (size + CHUNK_SIZE - 1) / CHUNK_SIZE
    }
}
