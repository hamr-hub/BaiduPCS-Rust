use crate::cloud_sync::error::{CloudSyncError, Result};
use crate::cloud_sync::types::{Connection, ConnectionConfig, JobStatus, SyncDirection, TransferJob};
use parking_lot::Mutex;
use rusqlite::{params, Connection as SqliteConn, Error as SqliteError, Row};
use std::sync::Arc;

pub struct CloudSyncPersistence { conn: Arc<Mutex<SqliteConn>> }

impl CloudSyncPersistence {
    pub fn open(db_path: &str) -> Result<Self> {
        let conn = SqliteConn::open(db_path)?;
        conn.execute_batch(r#"PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS cloud_sync_connections (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, config_json TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS cloud_sync_jobs (id TEXT PRIMARY KEY, name TEXT NOT NULL, source_connection_id TEXT NOT NULL, dest_connection_id TEXT NOT NULL, source_path TEXT NOT NULL, dest_path TEXT NOT NULL, direction TEXT NOT NULL, status TEXT NOT NULL, transferred_bytes INTEGER NOT NULL DEFAULT 0, total_bytes INTEGER NOT NULL DEFAULT 0, error TEXT, owner_uid INTEGER, local_cache_path TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, finished_at INTEGER);"#)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }
    pub fn insert_connection(&self, c: &Connection) -> Result<()> {
        let config_json = serde_json::to_string(&c.config)?;
        self.conn.lock().execute("INSERT INTO cloud_sync_connections (id, kind, name, config_json, created_at, updated_at) VALUES (?,?,?,?,?,?)", params![c.id, c.kind().as_str(), c.name(), config_json, c.created_at.timestamp(), c.updated_at.timestamp()])?;
        Ok(())
    }
    pub fn update_connection(&self, c: &Connection) -> Result<()> {
        let config_json = serde_json::to_string(&c.config)?;
        self.conn.lock().execute("UPDATE cloud_sync_connections SET config_json=?, name=?, updated_at=? WHERE id=?", params![config_json, c.name(), c.updated_at.timestamp(), c.id])?;
        Ok(())
    }
    pub fn delete_connection(&self, id: &str) -> Result<()> {
        let n = self.conn.lock().execute("DELETE FROM cloud_sync_connections WHERE id=?", params![id])?;
        if n == 0 { return Err(CloudSyncError::not_found(format!("连接不存在: {}", id))); }
        Ok(())
    }
    pub fn get_connection(&self, id: &str) -> Result<Option<Connection>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, kind, name, config_json, created_at, updated_at FROM cloud_sync_connections WHERE id=?")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(r) = rows.next()? { Ok(Some(row_to_connection(r)?)) } else { Ok(None) }
    }
    pub fn list_connections(&self) -> Result<Vec<Connection>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, kind, name, config_json, created_at, updated_at FROM cloud_sync_connections ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| row_to_connection(r))?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    }
    pub fn insert_job(&self, job: &TransferJob) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO cloud_sync_jobs (id, name, source_connection_id, dest_connection_id, source_path, dest_path, direction, status, transferred_bytes, total_bytes, error, owner_uid, local_cache_path, created_at, updated_at, finished_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                job.id, job.name, job.source_connection_id, job.dest_connection_id,
                job.source_path, job.dest_path, job.direction.as_str(), job.status.as_str(),
                job.transferred_bytes as i64, job.total_bytes as i64, job.error,
                job.owner_uid.map(|u| u as i64),
                job.local_cache_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                job.created_at.timestamp(), job.updated_at.timestamp(), job.finished_at.map(|d| d.timestamp()),
            ],
        )?;
        Ok(())
    }
    pub fn update_job(&self, job: &TransferJob) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE cloud_sync_jobs SET name=?, source_path=?, dest_path=?, status=?, transferred_bytes=?, total_bytes=?, error=?, finished_at=?, updated_at=? WHERE id=?",
            params![job.name, job.source_path, job.dest_path, job.status.as_str(), job.transferred_bytes as i64, job.total_bytes as i64, job.error, job.finished_at.map(|d| d.timestamp()), job.updated_at.timestamp(), job.id],
        )?;
        Ok(())
    }
    pub fn get_job(&self, id: &str) -> Result<Option<TransferJob>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name, source_connection_id, dest_connection_id, source_path, dest_path, direction, status, transferred_bytes, total_bytes, error, owner_uid, local_cache_path, created_at, updated_at, finished_at FROM cloud_sync_jobs WHERE id=?")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(r) = rows.next()? { Ok(Some(row_to_job(r)?)) } else { Ok(None) }
    }
    pub fn list_jobs(&self, limit: i64) -> Result<Vec<TransferJob>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name, source_connection_id, dest_connection_id, source_path, dest_path, direction, status, transferred_bytes, total_bytes, error, owner_uid, local_cache_path, created_at, updated_at, finished_at FROM cloud_sync_jobs ORDER BY updated_at DESC LIMIT ?")?;
        let rows = stmt.query_map(params![limit], |r| row_to_job(r))?;
        let mut out = Vec::new();
        for r in rows { out.push(r?); }
        Ok(out)
    }
    pub fn delete_job(&self, id: &str) -> Result<()> {
        let n = self.conn.lock().execute("DELETE FROM cloud_sync_jobs WHERE id=?", params![id])?;
        if n == 0 { return Err(CloudSyncError::not_found(format!("任务不存在: {}", id))); }
        Ok(())
    }
}

fn row_to_connection(r: &Row<'_>) -> std::result::Result<Connection, SqliteError> {
    let id: String = r.get(0)?;
    let _kind: String = r.get(1)?;
    let _name: String = r.get(2)?;
    let config_json: String = r.get(3)?;
    let created_ts: i64 = r.get(4)?;
    let updated_ts: i64 = r.get(5)?;
    let config: ConnectionConfig = serde_json::from_str(&config_json).map_err(|e| SqliteError::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?;
    let mk = |idx: usize, w: &str| SqliteError::FromSqlConversionFailure(idx, rusqlite::types::Type::Integer, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, w)));
    let created_at = chrono::DateTime::from_timestamp(created_ts, 0).ok_or_else(|| mk(4, "invalid created_at"))?;
    let updated_at = chrono::DateTime::from_timestamp(updated_ts, 0).ok_or_else(|| mk(5, "invalid updated_at"))?;
    Ok(Connection { id, config, created_at, updated_at })
}

fn row_to_job(r: &Row<'_>) -> std::result::Result<TransferJob, SqliteError> {
    let id: String = r.get(0)?;
    let name: String = r.get(1)?;
    let src: String = r.get(2)?;
    let dst: String = r.get(3)?;
    let sp: String = r.get(4)?;
    let dp: String = r.get(5)?;
    let dir_s: String = r.get(6)?;
    let st_s: String = r.get(7)?;
    let t: i64 = r.get(8)?;
    let tot: i64 = r.get(9)?;
    let err: Option<String> = r.get(10)?;
    let ou: Option<i64> = r.get(11)?;
    let lcp: Option<String> = r.get(12)?;
    let cts: i64 = r.get(13)?;
    let uts: i64 = r.get(14)?;
    let fts: Option<i64> = r.get(15)?;
    let direction = if dir_s == "upload" { SyncDirection::Upload } else { SyncDirection::Download };
    let status = match st_s.as_str() { "pending" => JobStatus::Pending, "running" => JobStatus::Running, "completed" => JobStatus::Completed, "cancelled" => JobStatus::Cancelled, _ => JobStatus::Failed };
    let mk = |idx: usize, w: &str| SqliteError::FromSqlConversionFailure(idx, rusqlite::types::Type::Integer, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, w)));
    let created_at = chrono::DateTime::from_timestamp(cts, 0).ok_or_else(|| mk(13, "invalid created_at"))?;
    let updated_at = chrono::DateTime::from_timestamp(uts, 0).ok_or_else(|| mk(14, "invalid updated_at"))?;
    let finished_at = fts.and_then(|t| chrono::DateTime::from_timestamp(t, 0));
    Ok(TransferJob {
        id, name, source_connection_id: src, dest_connection_id: dst,
        source_path: sp, dest_path: dp, direction, status,
        transferred_bytes: t.max(0) as u64, total_bytes: tot.max(0) as u64,
        error: err, created_at, updated_at, finished_at,
        local_cache_path: lcp.map(std::path::PathBuf::from),
        owner_uid: ou.map(|u| u as u64),
    })
}
