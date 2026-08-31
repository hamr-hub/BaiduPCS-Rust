//! 云同步 HTTP API（/api/v1/cloud-sync/*）

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::cloud_sync::{
    CloudSyncError, CloudSyncManager, Connection, CreateConnectionRequest, CreateJobRequest,
    JobSummary, ListObjectsResult, TestConnectionResult, TransferJob, UpdateConnectionRequest,
};
use crate::server::error::ApiError;
use crate::server::state::AppState;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self { success: true, data, error: None }
    }
}

async fn get_manager(state: &AppState) -> Result<Arc<CloudSyncManager>, ApiError> {
    let g = state.cloud_sync_manager.read().await;
    match &*g {
        Some(m) => Ok(Arc::clone(m)),
        None => Err(ApiError::BadRequest("云同步管理器未初始化".to_string())),
    }
}

fn map_err(e: CloudSyncError) -> ApiError {
    use crate::cloud_sync::ErrorCategory;
    let msg = e.to_string();
    match e.category() {
        ErrorCategory::Config | ErrorCategory::Auth | ErrorCategory::Forbidden | ErrorCategory::Cancelled => ApiError::BadRequest(msg),
        ErrorCategory::NotFound => ApiError::NotFound(msg),
        _ => ApiError::Internal(anyhow::anyhow!(msg)),
    }
}

pub async fn list_connections(State(state): State<AppState>) -> ApiResult<Json<ApiResponse<Vec<Connection>>>> {
    let m = get_manager(&state).await?;
    let list = m.list_connections().await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(list)))
}

pub async fn create_connection(State(state): State<AppState>, Json(req): Json<CreateConnectionRequest>) -> ApiResult<Json<ApiResponse<Connection>>> {
    let m = get_manager(&state).await?;
    let conn = m.create_connection(req).await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(conn)))
}

pub async fn get_connection(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<ApiResponse<Connection>>> {
    let m = get_manager(&state).await?;
    let conn = m.get_connection(&id).await.map_err(map_err)?
        .ok_or_else(|| ApiError::NotFound("连接不存在".to_string()))?;
    Ok(Json(ApiResponse::success(conn)))
}

pub async fn delete_connection(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<ApiResponse<serde_json::Value>>> {
    let m = get_manager(&state).await?;
    m.delete_connection(&id).await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(serde_json::json!({"deleted": id}))))
}

pub async fn test_connection(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<ApiResponse<TestConnectionResult>>> {
    let m = get_manager(&state).await?;
    let conn = m.get_connection(&id).await.map_err(map_err)?
        .ok_or_else(|| ApiError::NotFound("连接不存在".to_string()))?;
    let storage = crate::cloud_sync::build_storage(&conn.config, state.cloud_sync_baidu_resolver.as_ref()).await.map_err(map_err)?;
    let res = storage.test_connection().await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(res)))
}

#[derive(Debug, Deserialize)]
pub struct ListObjectsQuery { #[serde(default)] pub prefix: Option<String> }

pub async fn list_connection_objects(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<ListObjectsQuery>) -> ApiResult<Json<ApiResponse<ListObjectsResult>>> {
    let m = get_manager(&state).await?;
    let conn = m.get_connection(&id).await.map_err(map_err)?
        .ok_or_else(|| ApiError::NotFound("连接不存在".to_string()))?;
    let storage = crate::cloud_sync::build_storage(&conn.config, state.cloud_sync_baidu_resolver.as_ref()).await.map_err(map_err)?;
    let list = storage.list_objects(&q.prefix.unwrap_or_default()).await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(list)))
}

pub async fn list_jobs(State(state): State<AppState>) -> ApiResult<Json<ApiResponse<Vec<JobSummary>>>> {
    let m = get_manager(&state).await?;
    let list = m.list_jobs().await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(list)))
}

pub async fn create_job(State(state): State<AppState>, Json(req): Json<CreateJobRequest>) -> ApiResult<Json<ApiResponse<TransferJob>>> {
    let m = get_manager(&state).await?;
    let job = m.create_job(req).await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(job)))
}

pub async fn get_job(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<ApiResponse<TransferJob>>> {
    let m = get_manager(&state).await?;
    let job = m.get_job(&id).await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(job)))
}

pub async fn delete_job(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<ApiResponse<serde_json::Value>>> {
    let m = get_manager(&state).await?;
    m.delete_job(&id).await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(serde_json::json!({"deleted": id}))))
}

pub async fn cancel_job(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<ApiResponse<serde_json::Value>>> {
    let m = get_manager(&state).await?;
    m.cancel_job(&id).await.map_err(map_err)?;
    Ok(Json(ApiResponse::success(serde_json::json!({"cancelled": id}))))
}
