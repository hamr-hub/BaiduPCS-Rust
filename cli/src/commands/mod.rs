// commands 子模块集合。
//
// 每个子模块负责一类 HTTP API：
//   login    - 登录相关（cookie / qrcode / status / logout / whoami）
//   account  - 多账号管理
//   ls       - 列举网盘目录
//   file     - 文件 / 目录管理（mkdir / rm / mv / cp / rename / search）
//   upload   - 上传相关
//   download - 下载相关
//   share    - 分享转存
//   task     - 任务管理
//   config   - 配置读写
//
// 所有命令签名为 `async fn run(&ctx, args, printer) -> CliResult<()>`。
// 主分发逻辑见 main.rs。

pub mod account;
pub mod config;
pub mod download;
pub mod file;
pub mod login;
pub mod ls;
pub mod share;
pub mod task;
pub mod upload;
