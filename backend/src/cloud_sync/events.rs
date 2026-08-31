use serde::{Deserialize, Serialize};
use crate::cloud_sync::types::{JobStatus, SyncDirection};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CloudSyncEvent {
    StatusChanged {
        job_id: String, status: JobStatus,
        transferred_bytes: u64, total_bytes: u64,
        error: Option<String>,
    },
    Created {
        job_id: String, direction: SyncDirection,
        source_connection_id: String, dest_connection_id: String,
        source_path: String, dest_path: String,
    },
    Deleted { job_id: String },
}

impl CloudSyncEvent {
    pub fn event_type_name(&self) -> &'static str {
        match self {
            CloudSyncEvent::StatusChanged { .. } => "status_changed",
            CloudSyncEvent::Created { .. } => "created",
            CloudSyncEvent::Deleted { .. } => "deleted",
        }
    }
}
