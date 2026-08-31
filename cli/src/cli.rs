// CLI 顶层参数解析
//
// 设计原则：
// 1. 子命令与 HTTP API 一一对应，名字贴近 REST 风格。
// 2. 全局参数（--config / --account / --json / -q）在每个子命令都可覆盖（先 parse 再下发）。
// 3. 下载 / 上传 / 转存的「等待」语义：默认 --wait 同步等待到终态；--no-wait 仅入队即返回。
// 4. 复杂枚举（冲突策略）从 `baidu_netdisk_rust::uploader::conflict` 直接派生 clap 的 ValueEnum。

use std::path::PathBuf;

use baidu_netdisk_rust::uploader::conflict::{DownloadConflictStrategy, UploadConflictStrategy};
use clap::{Args, Parser, Subcommand, ValueEnum};

/// `baidu-pan-cli` — 与 HTTP API 等价的命令行入口。
#[derive(Debug, Parser)]
#[command(
    name = "baidu-pan-cli",
    version,
    about = "百度网盘 Rust 客户端命令行版本（驱动 baidu-netdisk-rust 核心库）",
    long_about = None,
    propagate_version = true,
)]
pub struct Cli {
    /// 配置文件路径（默认 `config/app.toml`，与服务器共用同一份）
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// 覆盖活跃账号 UID（不传则使用 accounts.json 中的活跃账号）
    #[arg(long, global = true, value_name = "UID")]
    pub account: Option<u64>,

    /// 所有输出走 JSON：stdout = 结果 JSON，stderr = NDJSON 进度事件
    #[arg(long, global = true)]
    pub json: bool,

    /// 安静模式：抑制 stderr 进度；stdout 仅输出最终结果
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// 显示详细日志（覆盖 RUST_LOG）
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// 登录相关
    #[command(subcommand)]
    Login(LoginCmd),

    /// 显示当前活跃账号
    Whoami,

    /// 账号管理
    #[command(subcommand)]
    Account(AccountCmd),

    /// 列出网盘目录
    Ls(LsArgs),

    /// 创建网盘目录
    Mkdir(MkdirArgs),

    /// 删除网盘文件 / 目录（一个或多个路径）
    Rm(RmArgs),

    /// 移动
    Mv(MvArgs),

    /// 复制
    Cp(CpArgs),

    /// 重命名
    Rename(RenameArgs),

    /// 搜索网盘文件
    Search(SearchArgs),

    /// 上传单个本地文件到网盘
    Upload(UploadArgs),

    /// 上传整个本地文件夹
    UploadFolder(UploadFolderArgs),

    /// 批量上传（多个 local=remote 对）
    UploadBatch(UploadBatchArgs),

    /// 下载网盘文件
    Download(DownloadArgs),

    /// 下载网盘文件夹
    DownloadFolder(DownloadFolderArgs),

    /// 转存分享链接
    #[command(subcommand)]
    Share(ShareCmd),

    /// 任务管理
    #[command(subcommand)]
    Task(TaskCmd),

    /// 配置读写
    #[command(subcommand)]
    Config(ConfigCmd),

    /// 健康检查：仅打印 AppState 是否就绪、活跃账号等
    Ping,
}

// ────────────────────────────────────────────────────────────────────
// login
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum LoginCmd {
    /// Cookie 登录：粘贴浏览器 DevTools 复制的完整 Cookie 字符串
    Cookie {
        /// 原始 Cookie 字符串，例如 "BDUSS=xxx; STOKEN=yyy;"
        raw: String,
    },
    /// 二维码登录：生成二维码 → 轮询 → 扫码成功后自动保存账号
    Qrcode {
        /// 轮询间隔（毫秒）
        #[arg(long, default_value_t = 1500)]
        poll_ms: u64,
        /// 最大等待时间（秒），0 表示无限等待
        #[arg(long, default_value_t = 0)]
        timeout_s: u64,
    },
    /// 单次查询二维码扫码状态（脚本场景）
    Status {
        /// 上一步 generate 返回的 sign
        sign: String,
    },
    /// 退出当前活跃账号（不删除账号记录）
    Logout,
}

// ────────────────────────────────────────────────────────────────────
// account
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum AccountCmd {
    /// 列出所有已登录账号（不含敏感字段）
    List,
    /// 切换活跃账号
    Switch { uid: u64 },
    /// 删除账号
    Delete { uid: u64 },
}

// ────────────────────────────────────────────────────────────────────
// 文件 / 目录
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct LsArgs {
    /// 网盘路径，例 `/foo/bar`
    pub path: String,
    /// 每页大小
    #[arg(long, default_value_t = 100)]
    pub page_size: u32,
    /// 翻页（1-based）
    #[arg(long, default_value_t = 1)]
    pub page: u32,
}

#[derive(Debug, Args)]
pub struct MkdirArgs {
    /// 网盘目录路径
    pub path: String,
}

#[derive(Debug, Args)]
pub struct RmArgs {
    /// 一个或多个网盘路径
    #[arg(required = true)]
    pub paths: Vec<String>,
}

#[derive(Debug, Args)]
pub struct MvArgs {
    /// 源网盘路径
    pub from: String,
    /// 目标父目录
    pub to: String,
    /// 目标文件名（默认与源同名）
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct CpArgs {
    pub from: String,
    pub to: String,
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct RenameArgs {
    pub path: String,
    pub newname: String,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    pub key: String,
    #[arg(long, default_value_t = 100)]
    pub num: u32,
}

// ────────────────────────────────────────────────────────────────────
// 上传 / 下载
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliUploadStrategy {
    SmartDedup,
    AutoRename,
    Overwrite,
}

impl From<CliUploadStrategy> for UploadConflictStrategy {
    fn from(v: CliUploadStrategy) -> Self {
        match v {
            CliUploadStrategy::SmartDedup => UploadConflictStrategy::SmartDedup,
            CliUploadStrategy::AutoRename => UploadConflictStrategy::AutoRename,
            CliUploadStrategy::Overwrite => UploadConflictStrategy::Overwrite,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliDownloadStrategy {
    Overwrite,
    Skip,
    AutoRename,
}

impl From<CliDownloadStrategy> for DownloadConflictStrategy {
    fn from(v: CliDownloadStrategy) -> Self {
        match v {
            CliDownloadStrategy::Overwrite => DownloadConflictStrategy::Overwrite,
            CliDownloadStrategy::Skip => DownloadConflictStrategy::Skip,
            CliDownloadStrategy::AutoRename => DownloadConflictStrategy::AutoRename,
        }
    }
}

#[derive(Debug, Args)]
pub struct UploadArgs {
    /// 本地文件路径
    pub local: PathBuf,
    /// 网盘目标路径（包含文件名），例如 `/backup/local.txt`
    pub remote: String,
    /// 启用客户端加密
    #[arg(long)]
    pub encrypt: bool,
    /// 冲突策略
    #[arg(long, value_enum)]
    pub conflict: Option<CliUploadStrategy>,
    /// 入队即返回，不等待上传完成
    #[arg(long)]
    pub no_wait: bool,
    /// 等待超时（秒）；0 = 无限
    #[arg(long, default_value_t = 0)]
    pub timeout_s: u64,
}

#[derive(Debug, Args)]
pub struct UploadFolderArgs {
    pub local_dir: PathBuf,
    pub remote_dir: String,
    #[arg(long)]
    pub encrypt: bool,
    #[arg(long, value_enum)]
    pub conflict: Option<CliUploadStrategy>,
    #[arg(long)]
    pub no_wait: bool,
    #[arg(long, default_value_t = 0)]
    pub timeout_s: u64,
}

#[derive(Debug, Args)]
pub struct UploadBatchArgs {
    /// 格式：`local_path=remote_path`，可重复多次
    #[arg(required = true, value_name = "LOCAL=REMOTE")]
    pub items: Vec<String>,
    #[arg(long)]
    pub encrypt: bool,
    #[arg(long, value_enum)]
    pub conflict: Option<CliUploadStrategy>,
    #[arg(long)]
    pub no_wait: bool,
}

#[derive(Debug, Args)]
pub struct DownloadArgs {
    /// 网盘文件路径
    pub remote: String,
    /// 本地保存路径（默认取配置文件中的 download_dir + filename）
    #[arg(long)]
    pub to: Option<PathBuf>,
    #[arg(long, value_enum)]
    pub conflict: Option<CliDownloadStrategy>,
    #[arg(long)]
    pub no_wait: bool,
    #[arg(long, default_value_t = 0)]
    pub timeout_s: u64,
}

#[derive(Debug, Args)]
pub struct DownloadFolderArgs {
    pub remote_dir: String,
    #[arg(long)]
    pub to: Option<PathBuf>,
    #[arg(long, value_enum)]
    pub conflict: Option<CliDownloadStrategy>,
    #[arg(long)]
    pub no_wait: bool,
}

// ────────────────────────────────────────────────────────────────────
// share
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum ShareCmd {
    /// 预览分享链接的文件列表（不入库）
    Preview {
        /// 分享 URL（含 surl）
        url: String,
        #[arg(long)]
        password: Option<String>,
    },
    /// 转存到自己的网盘
    Transfer {
        url: String,
        #[arg(long)]
        password: Option<String>,
        /// 保存到网盘的目标目录
        #[arg(long)]
        save: String,
        /// 转存后自动下载到本地
        #[arg(long)]
        auto_download: bool,
        #[arg(long)]
        no_wait: bool,
    },
}

// ────────────────────────────────────────────────────────────────────
// task
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum TaskCmd {
    /// 列出任务（默认全部；可用 --downloads/--uploads/--transfers 过滤）
    List {
        #[arg(long)]
        downloads: bool,
        #[arg(long)]
        uploads: bool,
        #[arg(long)]
        transfers: bool,
    },
    /// 查询任务状态
    Status { id: String },
    /// 暂停任务
    Pause { id: String },
    /// 恢复任务（从暂停或失败恢复）
    Resume { id: String },
    /// 取消任务（不删除已下载文件）
    Cancel { id: String },
    /// 同步等待任务到终态
    Wait {
        id: String,
        #[arg(long, default_value_t = 0)]
        timeout_s: u64,
    },
}

// ────────────────────────────────────────────────────────────────────
// config
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// 打印当前配置（JSON）
    Show,
    /// 重新从磁盘加载配置
    Reload,
}

/// 解析顶层 CLI。
pub fn parse_args() -> Cli {
    Cli::parse()
}
