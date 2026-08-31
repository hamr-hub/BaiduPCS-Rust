//! 云同步任务管理器

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::error;
use uuid::Uuid;

use crate::cloud_sync::error::{CloudSyncError, Result};
use crate::cloud_sync::events::CloudSyncEvent;
use crate::cloud_sync::persistence::CloudSyncPersistence;
use crate::cloud_sync::storage::{build_storage, BaiduClientResolver};
use crate::cloud_sync::types::{
    Connection, ConnectionConfig, CreateConnectionRequest, CreateJobRequest, JobStatus,
    JobSummary, StorageKind, SyncDirection, TransferJob, UpdateConnectionRequest,
};
use crate::server::events::TaskEvent;
use crate::server::websocket::WebSocketManager;

pub struct CloudSyncManager {
    persistence: Arc<CloudSyncPersistence>,
    baidu_resolver: Arc<dyn BaiduClientResolver>,
    ws: Arc<WebSocketManager>,
    cancel_flags: DashMap<String, Arc<Mutex<bool>>>,
    connection_cache: Mutex<HashMap<String, Arc<Connection>>>,
}

impl CloudSyncManager {
    pub fn new(persistence: Arc<CloudSyncPersistence>, baidu_resolver: Arc<dyn BaiduClientResolver>, ws: Arc<WebSocketManager>) -> Self {
        Self { persistence, baidu_resolver, ws, cancel_flags: DashMap::new(), connection_cache: Mutex::new(HashMap::new()) }
    }
    pub fn into_arc(self) -> Arc<Self> { Arc::new(self) }
    pub async fn list_connections(&self) -> Result<Vec<Connection>> { self.persistence.list_connections() }
    pub async fn get_connection(&self, id: &str) -> Result<Option<Connection>> {
        { let cache = self.connection_cache.lock().await; if let Some(c) = cache.get(id) { return Ok(Some((**c).clone())); } }
        let p = self.persistence.get_connection(id)?;
        if let Some(c) = &p { self.connection_cache.lock().await.insert(id.to_string(), Arc::new(c.clone())); }
        Ok(p)
    }
    pub async fn create_connection(&self, req: CreateConnectionRequest) -> Result<Connection> {
        let config = match req {
            CreateConnectionRequest::S3(c) => ConnectionConfig::S3(c),
            CreateConnectionRequest::Oss(c) => ConnectionConfig::Oss(c),
            CreateConnectionRequest::Baidu(c) => ConnectionConfig::Baidu(c),
        };
        if let ConnectionConfig::Baidu(ref c) = config {
            if self.baidu_resolver.resolve(c.owner_uid).await.is_none() {
                return Err(CloudSyncError::auth(format!("百度账号 {} 未登录", c.owner_uid)));
            }
        }
        let now = Utc::now();
        let conn = Connection { id: Uuid::new_v4().to_string(), config, created_at: now, updated_at: now };
        self.persistence.insert_connection(&conn)?;
        self.connection_cache.lock().await.insert(conn.id.clone(), Arc::new(conn.clone()));
        Ok(conn)
    }
    pub async fn update_connection(&self, id: &str, req: UpdateConnectionRequest) -> Result<Connection> {
        let mut conn = self.persistence.get_connection(id)?
            .ok_or_else(|| CloudSyncError::not_found("连接不存在"))?;
        apply_update(&mut conn.config, &req);
        conn.updated_at = Utc::now();
        self.persistence.update_connection(&conn)?;
        self.connection_cache.lock().await.insert(id.to_string(), Arc::new(conn.clone()));
        Ok(conn)
    }
    pub async fn delete_connection(&self, id: &str) -> Result<()> {
        self.persistence.delete_connection(id)?;
        self.connection_cache.lock().await.remove(id);
        Ok(())
    }
    pub async fn list_jobs(&self) -> Result<Vec<JobSummary>> {
        let jobs = self.persistence.list_jobs(500)?;
        let mut summaries = Vec::with_capacity(jobs.len());
        for job in jobs { summaries.push(self.to_summary(job).await?); }
        Ok(summaries)
    }
    pub async fn get_job(&self, id: &str) -> Result<TransferJob> {
        self.persistence.get_job(id)?.ok_or_else(|| CloudSyncError::not_found("任务不存在"))
    }
    pub async fn create_job(self: &Arc<Self>, req: CreateJobRequest) -> Result<TransferJob> {
        let src = self.get_connection(&req.source_connection_id).await?
            .ok_or_else(|| CloudSyncError::not_found(format!("源连接不存在: {}", req.source_connection_id)))?;
        let dst = self.get_connection(&req.dest_connection_id).await?
            .ok_or_else(|| CloudSyncError::not_found(format!("目标连接不存在: {}", req.dest_connection_id)))?;
        let direction = infer_direction(src.kind(), dst.kind());
        let now = Utc::now();
        let job = TransferJob {
            id: Uuid::new_v4().to_string(), name: req.name,
            source_connection_id: req.source_connection_id,
            dest_connection_id: req.dest_connection_id,
            source_path: req.source_path, dest_path: req.dest_path,
            direction, status: JobStatus::Pending,
            transferred_bytes: 0, total_bytes: 0, error: None,
            created_at: now, updated_at: now, finished_at: None,
            local_cache_path: None,
            owner_uid: req.owner_uid.or_else(|| extract_baidu_uid(&src, &dst)),
        };
        self.persistence.insert_job(&job)?;
        self.broadcast_event(&CloudSyncEvent::Created {
            job_id: job.id.clone(), direction,
            source_connection_id: job.source_connection_id.clone(),
            dest_connection_id: job.dest_connection_id.clone(),
            source_path: job.source_path.clone(),
            dest_path: job.dest_path.clone(),
        });
        Self::spawn_job_arc(self.clone(), job.clone());
        Ok(job)
    }
    pub async fn delete_job(&self, id: &str) -> Result<()> {
        if let Some(flag) = self.cancel_flags.get(id) { *flag.lock().await = true; }
        self.persistence.delete_job(id)?;
        self.broadcast_event(&CloudSyncEvent::Deleted { job_id: id.to_string() });
        Ok(())
    }
    pub async fn cancel_job(&self, id: &str) -> Result<()> {
        if let Some(flag) = self.cancel_flags.get(id) {
            *flag.lock().await = true;
        } else if let Some(mut job) = self.persistence.get_job(id)? {
            if !job.status.is_terminal() {
                job.status = JobStatus::Cancelled;
                job.updated_at = Utc::now();
                job.finished_at = Some(Utc::now());
                self.persistence.update_job(&job)?;
            }
        }
        Ok(())
    }
    pub fn spawn_job_arc(mgr: Arc<Self>, job: TransferJob) {
        let cancel = Arc::new(Mutex::new(false));
        mgr.cancel_flags.insert(job.id.clone(), cancel.clone());
        tokio::spawn(async move {
            let result = mgr.execute_job(job.clone(), cancel.clone()).await;
            if let Err(e) = result {
                error!(job_id = %job.id, "cloud_sync job failed: {}", e);
                let mut failed = job.clone();
                failed.status = JobStatus::Failed;
                failed.error = Some(format!("{:#}", e));
                failed.updated_at = Utc::now();
                failed.finished_at = Some(Utc::now());
                if let Err(e2) = mgr.persistence.update_job(&failed) {
                    error!("update failed job status error: {}", e2);
                }
                mgr.broadcast_event(&CloudSyncEvent::StatusChanged {
                    job_id: failed.id, status: failed.status,
                    transferred_bytes: failed.transferred_bytes, total_bytes: failed.total_bytes,
                    error: failed.error,
                });
            }
            mgr.cancel_flags.remove(&job.id);
        });
    }
    async fn execute_job(&self, mut job: TransferJob, cancel: Arc<Mutex<bool>>) -> Result<()> {
        job.status = JobStatus::Running;
        job.updated_at = Utc::now();
        self.persistence.update_job(&job)?;
        self.broadcast_event(&CloudSyncEvent::StatusChanged {
            job_id: job.id.clone(), status: job.status,
            transferred_bytes: job.transferred_bytes, total_bytes: job.total_bytes, error: None,
        });
        let src_conn = self.get_connection(&job.source_connection_id).await?
            .ok_or_else(|| CloudSyncError::not_found("源连接已不存在"))?;
        let dst_conn = self.get_connection(&job.dest_connection_id).await?
            .ok_or_else(|| CloudSyncError::not_found("目标连接已不存在"))?;
        let src_storage = build_storage(&src_conn.config, self.baidu_resolver.as_ref()).await?;
        let dst_storage = build_storage(&dst_conn.config, self.baidu_resolver.as_ref()).await?;
        let total = src_storage.head_size(&job.source_path).await.unwrap_or(0);
        job.total_bytes = total;
        self.persistence.update_job(&job)?;
        let cache = std::env::temp_dir().join(format!("csync-{}-{}", job.id, sanitize(&job.source_path)));
        if *cancel.lock().await { return Err(CloudSyncError::Cancelled); }
        src_storage.download_file(&job.source_path, &cache).await?;
        if *cancel.lock().await {
            let _ = tokio::fs::remove_file(&cache).await;
            return Err(CloudSyncError::Cancelled);
        }
        dst_storage.upload_file(&cache, &job.dest_path).await?;
        let _ = tokio::fs::remove_file(&cache).await;
        job.status = JobStatus::Completed;
        job.transferred_bytes = job.total_bytes;
        job.updated_at = Utc::now();
        job.finished_at = Some(Utc::now());
        self.persistence.update_job(&job)?;
        self.broadcast_event(&CloudSyncEvent::StatusChanged {
            job_id: job.id.clone(), status: job.status,
            transferred_bytes: job.transferred_bytes, total_bytes: job.total_bytes, error: None,
        });
        Ok(())
    }
    async fn to_summary(&self, job: TransferJob) -> Result<JobSummary> {
        let src_name = self.get_connection(&job.source_connection_id).await?
            .map(|c| c.name().to_string()).unwrap_or_else(|| "(已删除)".to_string());
        let dst_name = self.get_connection(&job.dest_connection_id).await?
            .map(|c| c.name().to_string()).unwrap_or_else(|| "(已删除)".to_string());
        Ok(JobSummary {
            id: job.id, name: job.name, direction: job.direction,
            source_connection_id: job.source_connection_id, source_connection_name: src_name,
            dest_connection_id: job.dest_connection_id, dest_connection_name: dst_name,
            source_path: job.source_path, dest_path: job.dest_path,
            status: job.status, transferred_bytes: job.transferred_bytes, total_bytes: job.total_bytes,
            error: job.error, created_at: job.created_at, updated_at: job.updated_at, finished_at: job.finished_at,
        })
    }
    fn broadcast_event(&self, ev: &CloudSyncEvent) {
        self.ws.send_if_subscribed(TaskEvent::CloudSync(ev.clone()), None);
    }
}

pub fn infer_direction(src: StorageKind, dst: StorageKind) -> SyncDirection {
    match (src, dst) {
        (_, StorageKind::Baidu) => SyncDirection::Upload,
        (StorageKind::Baidu, _) => SyncDirection::Download,
        _ => SyncDirection::Upload,
    }
}

fn extract_baidu_uid(src: &Connection, dst: &Connection) -> Option<u64> {
    use crate::cloud_sync::types::ConnectionConfig as CC;
    match (&src.config, &dst.config) {
        (CC::Baidu(c), _) => Some(c.owner_uid),
        (_, CC::Baidu(c)) => Some(c.owner_uid),
        _ => None,
    }
}

fn apply_update(cfg: &mut ConnectionConfig, req: &UpdateConnectionRequest) {
    use crate::cloud_sync::types::ConnectionConfig as CC;
    match cfg {
        CC::S3(c) => {
            if let Some(n) = &req.name { c.name = n.clone(); }
            if let Some(k) = &req.access_key { c.access_key = k.clone(); }
            if let Some(k) = &req.secret_key { c.secret_key = k.clone(); }
            if let Some(r) = &req.region { c.region = r.clone(); }
            if let Some(b) = &req.bucket { c.bucket = b.clone(); }
            if let Some(e) = &req.endpoint { c.endpoint = Some(e.clone()); }
            if let Some(p) = req.path_style { c.path_style = Some(p); }
        }
        CC::Oss(c) => {
            if let Some(n) = &req.name { c.name = n.clone(); }
            if let Some(k) = &req.access_key { c.access_key = k.clone(); }
            if let Some(k) = &req.secret_key { c.secret_key = k.clone(); }
            if let Some(r) = &req.region { c.region = r.clone(); }
            if let Some(b) = &req.bucket { c.bucket = b.clone(); }
            if let Some(e) = &req.endpoint { c.endpoint = Some(e.clone()); }
            if let Some(ie) = &req.internal_endpoint { c.internal_endpoint = Some(ie.clone()); }
            if let Some(p) = req.path_style { c.path_style = Some(p); }
        }
        CC::Baidu(c) => {
            if let Some(n) = &req.name { c.name = n.clone(); }
            if let Some(u) = req.owner_uid { c.owner_uid = u; }
        }
    }
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}
