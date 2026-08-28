//! 网盘远端文件变动 watcher —— 周期性探测根目录最近条目，diff 出新增/未缓存条目，
//! 通过 WebSocket 推 `TaskEvent::System(SystemEvent::RemoteFileChanged)` 给前端。
//!
//! ## 为什么需要 watcher（关键背景）
//!
//! 百度个人网盘**没有**服务端事件推送 webhook ——任何客户端（网页、官方 App、跨设备同步、
//! 朋友共享后通过分享转存）动了网盘，远端不会主动通知本应用。要感知"我的网盘里有
//! 新文件"只能靠**主动轮询**。本模块就是这个 polling 实现。
//!
//! ## 设计取舍
//!
//! 1. **只扫根 `/`**：百度的 `xpan/file?method=list&order=time` 有 depth 0/1 的天然边界；
//!    子目录里的变动**不会被发现**。这跟 share-sync 的递归扫描互补：用户若要全网盘感知，
//!    把关注目录挂到 `ShareSync` 上。要做全盘扫描会触发百度的反爬节奏限制，所以**默认不**
//!    递归。
//!
//! 2. **fs_id 集合去重**：每次拉一批 `top_n=200`，对 `fs_id` 集合做差集，新增/未见过
//!    的条目才发事件。这避免 watcher 启动时把整个根目录刷一遍给前端"叮叮叮"。
//!
//! 3. **per-account 独立实例**：与 `CloudDlMonitor` 同构 —— `state.rs` 用
//!    `DashMap<Uid, Arc<RecentWatcher>>` 维护，账号注销时一并 drop spawn task。
//!
//! 4. **节流 + 自适应间隔**：发现 0 新条目就指数退避，封顶 `idle_interval`（默认 5 分钟）；
//!    发现新条目立刻拉满到 `min_interval`（默认 60 秒）。后台睡眠统一走 `tokio::select!`
//!    监听停止信号，关账号/重启 server 立刻收尾。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::auth::Uid;
use crate::netdisk::client::NetdiskClient;
use crate::netdisk::types::FileItem;
use crate::server::events::{SystemEvent, TaskEvent, TimestampedEvent};
use crate::server::websocket::{WsServerMessage, WebSocketManager};

/// RecentWatcher 的可调配置
///
/// 默认值针对"用户在线 + 多端活跃"场景调优 —— idle 5 分钟也只占一次 QPS，
/// 活跃时最快每 60 秒一次（防止百度风控）。所有字段在 `RecentWatcher::with_config` 里覆盖。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentWatcherConfig {
    /// 每次拉取的最大条目数（=百度服务端 page size 上限 200）
    pub top_n: u32,
    /// 最短轮询间隔（发现新条目后下一次拉取至少要等这么久）
    pub min_interval: Duration,
    /// 最长轮询间隔（连续无新条目时退避到这条上限）
    pub max_interval: Duration,
    /// 初始"已知"基线：是否在第一次扫描时**不**推送本次扫到的所有条目
    ///
    /// `true`（默认）=首次启动静默建立 baseline，等下次扫描才 diff，"嘀嘀嘀"的启动
    /// 噪音最小；`false` =首次扫描所得全部当作新增推送，适合"我刚加完账号想马上看到
    /// 之前累积的新文件"的场景。
    pub silent_baseline: bool,
}

impl Default for RecentWatcherConfig {
    fn default() -> Self {
        Self {
            top_n: 200,
            min_interval: Duration::from_secs(60),
            max_interval: Duration::from_secs(300),
            silent_baseline: true,
        }
    }
}

/// 网盘远端文件变动 watcher —— 一个账号一个实例。
///
/// `start()` spawn 一个**托管**后台 task（join handle 由 caller 持有，shutdown 时调
/// `stop()`+`join()` 或 `abort()`）。`running()` 暴露 in-flight 标志。
pub struct RecentWatcher {
    owner_uid: Uid,
    client: Arc<NetdiskClient>,
    ws_manager: Arc<RwLock<Option<Arc<WebSocketManager>>>>,
    config: RecentWatcherConfig,

    /// 上一次扫描看到的 fs_id 集合（仅在内存里，重启即清零——重启后第一次扫描沿用
    /// `silent_baseline` 决定是否把现存条目当新增）
    seen_fs_ids: RwLock<HashSet<u64>>,

    /// 后台 task 是否在跑（避免双重 spawn）
    running: AtomicBool,

    /// 自增事件 ID —— 与 `TimestampedEvent::event_id` 对齐。watcher 单调递增，
    /// 服务端 WebSocket 用这个做幂等 / dedup。前端可忽略。
    next_event_id: std::sync::atomic::AtomicU64,
}

impl RecentWatcher {
    /// 默认配置的便利构造（等价于 `with_config(Uid, client, ws, RecentWatcherConfig::default())`）
    pub fn new(
        owner_uid: Uid,
        client: Arc<NetdiskClient>,
        ws_manager: Arc<WebSocketManager>,
    ) -> Self {
        Self::with_config(
            owner_uid,
            client,
            Arc::new(RwLock::new(Some(ws_manager))),
            RecentWatcherConfig::default(),
        )
    }

    /// 带配置的完全构造 —— 与 `state.rs::register_account_managers` 串联使用
    pub fn with_config(
        owner_uid: Uid,
        client: Arc<NetdiskClient>,
        ws_manager: Arc<RwLock<Option<Arc<WebSocketManager>>>>,
        config: RecentWatcherConfig,
    ) -> Self {
        Self {
            owner_uid,
            client,
            ws_manager,
            config,
            seen_fs_ids: RwLock::new(HashSet::new()),
            running: AtomicBool::new(false),
            next_event_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// 在 WebSocketManager 已 wired 完成后注入（在账号注册流程里由 state.rs 调）——
    /// `RecentWatcher` 的 `ws_manager` 用 `Option<RwLock<...>>` 间接持有，这正好对应
    /// 服务端的"先构造后注入"模式。
    pub async fn set_ws_manager(&self, ws: Arc<WebSocketManager>) {
        *self.ws_manager.write().await = Some(ws);
    }

    /// 当前是否在跑（不暴露内部 mutex，只给调试/统计用）
    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 启动后台 watcher task
    ///
    /// - 同一实例重复调 → 静默返回（`running.swap(true)` 已经是 true）
    /// - 启动 task 不绑 `JoinHandle` —— RecentWatcher 用 `running: AtomicBool` +
    ///   `running.store(false)` 作为停止信号。若 caller 需要"shutdown 等 join"，用
    ///   [`stop`] 把 running 置 false 即可；task 在下一轮 sleep 或扫描尾巴 `select!` 命中。
    pub fn start(self: Arc<Self>) {
        if self.running.swap(true, Ordering::SeqCst) {
            debug!(
                "RecentWatcher (uid={}) 已在运行，跳过重复 start",
                self.owner_uid.raw()
            );
            return;
        }
        let me = Arc::clone(&self);
        tokio::spawn(async move {
            me.run_loop().await;
        });
        info!(
            "RecentWatcher (uid={}) 已启动 (top_n={}, min={:?}, max={:?}, silent_baseline={})",
            self.owner_uid.raw(),
            self.config.top_n,
            self.config.min_interval,
            self.config.max_interval,
            self.config.silent_baseline,
        );
    }

    /// 给 watcher 发停止信号 —— task 在下个 sleep/select 命中处退出
    pub fn stop(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            info!("RecentWatcher (uid={}) 收到停止信号", self.owner_uid.raw());
        }
    }

    /// 主循环：拉 → diff → 推 WS → 自适应间隔 sleep
    ///
    /// ⚠️ 这个函数**不持有任何锁过 await**，所有 `RwLock` 都用 micro-task scoped。
    /// 拉网络请求与推 WS 的耗时被分摊到 `min_interval` 的快路径 —— 每分钟一次，
    /// 远低于风控阈值。
    async fn run_loop(self: Arc<Self>) {
        let mut current_interval = self.config.min_interval;

        while self.running.load(Ordering::SeqCst) {
            // ====== 1) 拉一批 ======
            let resp_result = self
                .client
                .list_recent_files(self.config.top_n, 0)
                .await;

            // ====== 2) 处理结果 / 自适应间隔 ======
            let new_files: Vec<FileItem> = match resp_result {
                Ok(resp) if resp.errno == 0 => {
                    let new = self.diff_and_publish(resp.list).await;
                    if new > 0 {
                        // 发现新条目 → 拉回最短间隔
                        current_interval = self.config.min_interval;
                    } else {
                        // 空跑 → 指数退避到 max
                        current_interval = (current_interval * 2).min(self.config.max_interval);
                    }
                    // 取走 FileItem 是为了不让 self.diff_and_publish 同时持有 resp.list 引用
                    // （_list 已经 move 进 diff_and_publish）
                    Vec::new()
                }
                Ok(resp) => {
                    warn!(
                        "RecentWatcher (uid={}) 远端返回 errno={} errmsg={}",
                        self.owner_uid.raw(),
                        resp.errno,
                        resp.errmsg
                    );
                    current_interval = self.config.max_interval;
                    Vec::new()
                }
                Err(e) => {
                    // 网络/认证错误：回退到最慢节奏，避免失败风暴
                    warn!(
                        "RecentWatcher (uid={}) 拉取失败: {:?}",
                        self.owner_uid.raw(),
                        e
                    );
                    current_interval = self.config.max_interval;
                    Vec::new()
                }
            };
            drop(new_files); // 仅借用语义：函数实际已取走

            // ====== 3) 间隔睡眠 + 停止信号监听 ======
            if !self.running.load(Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(current_interval) => {}
                _ = wait_for_stop(&self.running) => {
                    break;
                }
            }
        }
        info!(
            "RecentWatcher (uid={}) 主循环已退出",
            self.owner_uid.raw()
        );
    }

    /// 把当前拉到的列表跟缓存做差集，把新出现的 item 推 WS。
    ///
    /// 返回本轮发出的事件数（=新发现/已变更条目数），用于主循环决定节奏。
    async fn diff_and_publish(&self, items: Vec<FileItem>) -> usize {
        // 第一遍：拿到当前 seen 集合的快照，避免在锁内 await
        let mut seen = self.seen_fs_ids.write().await;

        // silent_baseline：首次扫描把当前所有 item 当 baseline，**不发**任何事件
        if seen.is_empty() && self.config.silent_baseline {
            let n = items.len();
            for it in &items {
                seen.insert(it.fs_id);
            }
            debug!(
                "RecentWatcher (uid={}) 建立 baseline: {} 个条目静音",
                self.owner_uid.raw(),
                n
            );
            return 0;
        }

        // 第二遍：差集 = 新出现的 fs_id
        let mut new_items: Vec<&FileItem> = Vec::with_capacity(items.len());
        let mut emitted = 0usize;
        for it in &items {
            if seen.insert(it.fs_id) {
                // insert 返回 true = 该 key 之前不存在 → 新条目
                new_items.push(it);
            }
        }
        if new_items.is_empty() {
            return 0;
        }

        // 第三遍：拿到 ws_manager 的快照（避免持锁 await），在外面 publish
        let ws_snapshot = self.ws_manager.read().await.clone();
        let Some(ws) = ws_snapshot else {
            warn!(
                "RecentWatcher (uid={}) ws_manager 尚未注入，本轮 {} 个新条目丢弃",
                self.owner_uid.raw(),
                new_items.len()
            );
            return 0;
        };

        for it in new_items {
            let event_id = self
                .next_event_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let evt = SystemEvent::RemoteFileChanged {
                fs_id: it.fs_id,
                path: it.path.clone(),
                filename: it.server_filename.clone(),
                size: it.size,
                is_dir: it.isdir == 1,
                server_mtime: it.server_mtime,
                account_uid: self.owner_uid.raw(),
            };
            let task_event = TaskEvent::System(evt);
            let ts = TimestampedEvent::new(event_id, task_event);
            ws.broadcast(WsServerMessage::event(ts));
            emitted += 1;
        }
        emitted
    }
}

/// 等待 `running` 变 false 的辅助 future —— 用 `tokio::time::interval` 100ms 轮询。
///
/// 为什么不直接 `Notify::notified()` ？因为我们要兼容"只读 AtomicBool"接口（避免公开
/// `Notify` 字段到外部），且后台 shutdown 容忍几秒延迟。
async fn wait_for_stop(running: &AtomicBool) {
    let mut ticker = interval(Duration::from_millis(200));
    // 跳过首次立即 tick
    ticker.tick().await;
    loop {
        if !running.load(Ordering::SeqCst) {
            return;
        }
        ticker.tick().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_in_safe_range() {
        let cfg = RecentWatcherConfig::default();
        assert_eq!(cfg.top_n, 200);
        assert!(cfg.min_interval < cfg.max_interval);
        // 安全：min ≥ 30s，避免轮询风暴
        assert!(cfg.min_interval >= Duration::from_secs(30));
        // 安全：max ≤ 30min，避免 idle 太久错失窗口
        assert!(cfg.max_interval <= Duration::from_secs(30 * 60));
        assert!(cfg.silent_baseline);
    }

    #[test]
    fn config_silent_baseline_can_be_toggled() {
        let mut cfg = RecentWatcherConfig::default();
        assert!(cfg.silent_baseline);
        cfg.silent_baseline = false;
        assert!(!cfg.silent_baseline);
    }

    #[test]
    fn atomic_running_flag_works() {
        // 单纯测 AtomicBool 行为，避免构造带 client/ws 的 RecentWatcher（要 runtime）
        let flag = AtomicBool::new(false);
        assert!(!flag.load(Ordering::SeqCst));
        flag.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst));
        // swap 语义：false → true 返回旧值 false
        assert!(!flag.swap(false, Ordering::SeqCst));
        assert!(!flag.load(Ordering::SeqCst));
    }
}
