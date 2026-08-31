//! 云同步子系统核心类型

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageKind { S3, Oss, Baidu }

impl StorageKind {
    pub fn as_str(&self) -> &'static str {
        match self { StorageKind::S3 => "s3", StorageKind::Oss => "oss", StorageKind::Baidu => "baidu" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ConnectionConfig { S3(S3Config), Oss(OssConfig), Baidu(BaiduConfig) }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    pub name: String, pub region: String, pub bucket: String,
    pub access_key: String, pub secret_key: String,
    #[serde(default)] pub endpoint: Option<String>,
    #[serde(default)] pub path_style: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OssConfig {
    pub name: String, pub region: String, pub bucket: String,
    pub access_key: String, pub secret_key: String,
    #[serde(default)] pub endpoint: Option<String>,
    #[serde(default)] pub internal_endpoint: Option<String>,
    #[serde(default)] pub path_style: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaiduConfig { pub name: String, pub owner_uid: u64 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub id: String, pub config: ConnectionConfig,
    pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
}

impl Connection {
    pub fn kind(&self) -> StorageKind {
        match &self.config {
            ConnectionConfig::S3(_) => StorageKind::S3,
            ConnectionConfig::Oss(_) => StorageKind::Oss,
            ConnectionConfig::Baidu(_) => StorageKind::Baidu,
        }
    }
    pub fn name(&self) -> &str {
        match &self.config {
            ConnectionConfig::S3(c) => &c.name,
            ConnectionConfig::Oss(c) => &c.name,
            ConnectionConfig::Baidu(c) => &c.name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncDirection { Upload, Download }

impl SyncDirection {
    pub fn as_str(&self) -> &'static str {
        match self { SyncDirection::Upload => "upload", SyncDirection::Download => "download" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CreateConnectionRequest { S3(S3Config), Oss(OssConfig), Baidu(BaiduConfig) }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateConnectionRequest {
    #[serde(default)] pub name: Option<String>,
    #[serde(default)] pub access_key: Option<String>,
    #[serde(default)] pub secret_key: Option<String>,
    #[serde(default)] pub region: Option<String>,
    #[serde(default)] pub bucket: Option<String>,
    #[serde(default)] pub endpoint: Option<String>,
    #[serde(default)] pub internal_endpoint: Option<String>,
    #[serde(default)] pub path_style: Option<bool>,
    #[serde(default)] pub owner_uid: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferJob {
    pub id: String, pub name: String,
    pub source_connection_id: String, pub dest_connection_id: String,
    pub source_path: String, pub dest_path: String,
    pub direction: SyncDirection, pub status: JobStatus,
    pub transferred_bytes: u64, pub total_bytes: u64,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub local_cache_path: Option<PathBuf>, pub owner_uid: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus { Pending, Running, Completed, Failed, Cancelled }

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Pending => "pending", JobStatus::Running => "running",
            JobStatus::Completed => "completed", JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub source_connection_id: String, pub dest_connection_id: String,
    pub source_path: String, pub dest_path: String,
    #[serde(default)] pub owner_uid: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: String, pub name: String, pub direction: SyncDirection,
    pub source_connection_id: String, pub source_connection_name: String,
    pub dest_connection_id: String, pub dest_connection_name: String,
    pub source_path: String, pub dest_path: String,
    pub status: JobStatus,
    pub transferred_bytes: u64, pub total_bytes: u64,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub ok: bool, pub latency_ms: u64,
    pub error: Option<String>, pub sample_objects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub key: String, pub size: u64,
    pub last_modified: Option<DateTime<Utc>>, pub etag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListObjectsResult {
    pub objects: Vec<ObjectInfo>, pub prefixes: Vec<String>, pub truncated: bool,
}
