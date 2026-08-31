// 启动上下文：装载 AppState + LogGuard + 当前活跃账号的便捷访问。
//
// 启动流程见 plan：
//   1. AppConfig::load_or_default
//   2. logging::init_logging —— 必须保持 guard 直到 main 退出
//   3. AppState::new()
//   4. state.load_initial_session()
//   5. 把 (state, cfg, log_guard) 包成 BootContext，传给所有 commands

use std::sync::Arc;

use baidu_netdisk_rust::{
    auth::Uid,
    config::AppConfig,
    logging::LogGuard,
    server::AppState,
};
use tokio::sync::RwLock;

use crate::error::{CliError, CliResult};

/// 全局启动上下文。所有子命令共享。
#[derive(Clone)]
pub struct BootContext {
    pub state: Arc<AppState>,
    pub cfg: Arc<RwLock<AppConfig>>,
    /// 仅持有用于延长 logging drop；业务代码不应调用。
    pub _log_guard: Arc<LogGuard>,
}

impl BootContext {
    /// 取当前生效的 uid：
    /// 1. CLI --account 覆盖（每次命令的临时 uid）
    /// 2. accounts.json 中的活跃 uid
    /// 3. 否则返回 NotLoggedIn
    pub async fn effective_uid(&self, override_uid: Option<u64>) -> CliResult<Uid> {
        if let Some(uid) = override_uid {
            // 校验 uid 在 accounts.json 中存在
            let am = self.state.account_manager.lock().await;
            if am.get_user(Uid::new(uid)).is_none() {
                return Err(CliError::UnknownAccount(uid));
            }
            return Ok(Uid::new(uid));
        }

        let active = *self.state.active_uid.read().await;
        match active {
            Some(u) => Ok(u),
            None => Err(CliError::NotLoggedIn),
        }
    }

    /// 取当前活跃账号的 UserAuth（仅在 override_uid=None 时调用）
    pub async fn active_user_auth(&self) -> CliResult<baidu_netdisk_rust::auth::UserAuth> {
        self.state.active_user_auth().await.ok_or(CliError::NotLoggedIn)
    }

    /// 取活跃 NetdiskClient
    pub async fn active_client(&self) -> CliResult<Arc<baidu_netdisk_rust::netdisk::NetdiskClient>> {
        self.state.active_client().await.ok_or(CliError::NotLoggedIn)
    }

    /// 按 uid 取 NetdiskClient（per-uid 池查找）
    pub async fn client_for(&self, uid: Uid) -> CliResult<Arc<baidu_netdisk_rust::netdisk::NetdiskClient>> {
        self.state
            .client_pool
            .read()
            .await
            .get_client(uid)
            .ok_or(CliError::UnknownAccount(uid.raw()))
    }

    /// 按 uid 取 DownloadManager
    pub fn download_manager_for(&self, uid: Uid) -> CliResult<Arc<baidu_netdisk_rust::DownloadManager>> {
        self.state
            .download_manager_for(uid)
            .ok_or(CliError::UnknownAccount(uid.raw()))
    }

    /// 按 uid 取 UploadManager
    pub fn upload_manager_for(&self, uid: Uid) -> CliResult<Arc<baidu_netdisk_rust::UploadManager>> {
        self.state
            .upload_manager_for(uid)
            .ok_or(CliError::UnknownAccount(uid.raw()))
    }

    /// 按 uid 取 TransferManager
    pub fn transfer_manager_for(&self, uid: Uid) -> CliResult<Arc<baidu_netdisk_rust::TransferManager>> {
        self.state
            .transfer_manager_for(uid)
            .ok_or(CliError::UnknownAccount(uid.raw()))
    }

    /// 按 task_id 反查所属 DownloadManager（跨账号）
    pub async fn find_download_manager(
        &self,
        task_id: &str,
    ) -> CliResult<(Uid, Arc<baidu_netdisk_rust::DownloadManager>)> {
        self.state
            .find_download_manager_for_task(task_id)
            .await
            .ok_or_else(|| CliError::TaskNotFound(task_id.to_string()))
    }

    /// 按 task_id 反查所属 UploadManager
    pub async fn find_upload_manager(
        &self,
        task_id: &str,
    ) -> CliResult<(Uid, Arc<baidu_netdisk_rust::UploadManager>)> {
        self.state
            .find_upload_manager_for_task(task_id)
            .await
            .ok_or_else(|| CliError::TaskNotFound(task_id.to_string()))
    }

    /// 按 task_id 反查所属 TransferManager
    pub async fn find_transfer_manager(
        &self,
        task_id: &str,
    ) -> CliResult<(Uid, Arc<baidu_netdisk_rust::TransferManager>)> {
        self.state
            .find_transfer_manager_for_task(task_id)
            .await
            .ok_or_else(|| CliError::TaskNotFound(task_id.to_string()))
    }
}
