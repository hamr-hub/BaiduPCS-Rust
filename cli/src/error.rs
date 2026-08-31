// CLI 错误类型与退出码
//
// 借鉴 decrypt-cli/src/main.rs 的做法：所有命令统一返回 `Result<T, CliError>`，
// main() 把 CliError 映射成 std::process::ExitCode 退出码。

use std::process::ExitCode;

use thiserror::Error;

/// CLI 错误。所有命令统一返回这个。
#[derive(Debug, Error)]
pub enum CliError {
    #[error("未登录：请先运行 `baidu-pan-cli login cookie ...` 或 `login qrcode`")]
    NotLoggedIn,

    #[error("无活跃账号：多账号场景下请用 `--account <UID>` 显式指定")]
    NoActiveAccount,

    #[error("账号 uid={0} 不存在")]
    UnknownAccount(u64),

    #[error("找不到任务：{0}")]
    TaskNotFound(String),

    #[error("任务超时：{0}")]
    TaskTimeout(String),

    #[error("任务失败：{0}")]
    TaskFailed(String),

    #[error("参数错误：{0}")]
    BadArgument(String),

    #[error("本地路径不存在：{0}")]
    LocalPathMissing(String),

    #[error("远程路径不合法：{0}")]
    RemotePathInvalid(String),

    #[error("核心库错误：{0:#}")]
    Core(#[from] anyhow::Error),

    #[error("JSON 序列化错误：{0}")]
    Json(#[from] serde_json::Error),

    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

impl CliError {
    /// 转成进程退出码。约定：
    /// - 0  success（不要从 CliError 出来）
    /// - 1  通用运行时错误（Io / Core）
    /// - 2  参数 / 用法错误
    /// - 3  鉴权 / 账号问题
    /// - 4  任务状态异常（不存在 / 超时 / 失败）
    /// - 5  IO / 路径不存在
    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::NotLoggedIn | CliError::NoActiveAccount | CliError::UnknownAccount(_) => 3,
            CliError::TaskNotFound(_) | CliError::TaskTimeout(_) | CliError::TaskFailed(_) => 4,
            CliError::BadArgument(_) | CliError::RemotePathInvalid(_) => 2,
            CliError::LocalPathMissing(_) | CliError::Io(_) => 5,
            CliError::Core(_) | CliError::Json(_) => 1,
        }
    }
}

impl From<CliError> for ExitCode {
    fn from(err: CliError) -> ExitCode {
        // 先打印到 stderr（main 还会再打一次，但这里兜底，比如 printer 关闭后还出错）。
        eprintln!("error: {err}");
        ExitCode::from(err.exit_code())
    }
}

/// 业务逻辑返回 Result 的便捷别名。
pub type CliResult<T> = std::result::Result<T, CliError>;
