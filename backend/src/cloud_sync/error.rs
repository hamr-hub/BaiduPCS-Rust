use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory { Config, Network, Auth, NotFound, Forbidden, RateLimited, Internal, Cancelled }

#[derive(Debug, Error)]
pub enum CloudSyncError {
    #[error("配置错误: {0}")] Config(String),
    #[error("网络错误: {0}")] Network(String),
    #[error("鉴权失败: {0}")] Auth(String),
    #[error("资源不存在: {0}")] NotFound(String),
    #[error("权限拒绝: {0}")] Forbidden(String),
    #[error("触发限流: {0}")] RateLimited(String),
    #[error("内部错误: {0}")] Internal(String),
    #[error("用户取消")] Cancelled,
    #[error("未知错误: {0}")] Other(String),
}

impl CloudSyncError {
    pub fn category(&self) -> ErrorCategory {
        match self {
            CloudSyncError::Config(_) => ErrorCategory::Config,
            CloudSyncError::Network(_) => ErrorCategory::Network,
            CloudSyncError::Auth(_) => ErrorCategory::Auth,
            CloudSyncError::NotFound(_) => ErrorCategory::NotFound,
            CloudSyncError::Forbidden(_) => ErrorCategory::Forbidden,
            CloudSyncError::RateLimited(_) => ErrorCategory::RateLimited,
            CloudSyncError::Cancelled => ErrorCategory::Cancelled,
            _ => ErrorCategory::Internal,
        }
    }
    pub fn config<S: Into<String>>(m: S) -> Self { Self::Config(m.into()) }
    pub fn network<S: Into<String>>(m: S) -> Self { Self::Network(m.into()) }
    pub fn auth<S: Into<String>>(m: S) -> Self { Self::Auth(m.into()) }
    pub fn not_found<S: Into<String>>(m: S) -> Self { Self::NotFound(m.into()) }
    pub fn forbidden<S: Into<String>>(m: S) -> Self { Self::Forbidden(m.into()) }
    pub fn rate_limited<S: Into<String>>(m: S) -> Self { Self::RateLimited(m.into()) }
    pub fn internal<S: Into<String>>(m: S) -> Self { Self::Internal(m.into()) }
}

impl From<anyhow::Error> for CloudSyncError { fn from(e: anyhow::Error) -> Self { Self::Internal(format!("{:#}", e)) } }
impl From<std::io::Error> for CloudSyncError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => Self::NotFound(e.to_string()),
            std::io::ErrorKind::PermissionDenied => Self::Forbidden(e.to_string()),
            _ => Self::Internal(e.to_string()),
        }
    }
}
impl From<rusqlite::Error> for CloudSyncError {
    fn from(e: rusqlite::Error) -> Self {
        match e { rusqlite::Error::QueryReturnedNoRows => Self::NotFound("记录不存在".to_string()), _ => Self::Internal(format!("数据库错误: {}", e)) }
    }
}
impl From<serde_json::Error> for CloudSyncError { fn from(e: serde_json::Error) -> Self { Self::Internal(format!("序列化错误: {}", e)) } }

pub type Result<T> = std::result::Result<T, CloudSyncError>;
