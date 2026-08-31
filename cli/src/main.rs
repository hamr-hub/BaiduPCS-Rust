// baidu-pan-cli 主入口。
//
// 启动流程（与 plan 一致）：
//   1) 解析 CLI 参数
//   2) 初始化日志（init_logging 必须一次；guard 持到 main 末尾）
//   3) load_or_default AppConfig
//   4) AppState::new + load_initial_session
//   5) 分发到 commands::*::run

#![deny(rust_2018_idioms)]
#![warn(unused_must_use)]

mod cli;
mod commands;
mod context;
mod error;
mod output;
mod wait;

use std::process::ExitCode;
use std::sync::Arc;

use baidu_netdisk_rust::{config::AppConfig, logging, server::AppState};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::cli::{Cli, Commands};
use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::{OutputMode, Printer};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let cli = cli::parse_args();
    let printer = Printer::new(
        if cli.json { OutputMode::Json } else { OutputMode::Human },
        cli.quiet,
    );

    match run(cli, printer).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            printer.result_json(&serde_json::json!({
                "ok": false,
                "code": e.exit_code(),
                "error": e.to_string(),
            }));
            e.exit_code().into()
        }
    }
}

async fn run(cli: Cli, printer: Printer) -> CliResult<()> {
    // ── 1) 加载配置 ─────────────────────────────────────────────────
    let cfg_path = cli
        .config
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config/app.toml".to_string());

    let cfg = AppConfig::load_or_default(&cfg_path).await;

    // ── 2) 日志（一次性，guard 持到退出）────────────────────────────
    // -v 会覆盖 LogConfig 的 default level
    let mut log_cfg = cfg.log.clone();
    if cli.verbose {
        log_cfg.level = "debug".to_string();
    }
    let log_guard = logging::init_logging(&log_cfg);
    let log_guard = Arc::new(log_guard);

    info!(
        "baidu-pan-cli 启动: config={}, json={}, quiet={}",
        cfg_path, cli.json, cli.quiet
    );

    // ── 3) AppState + 会话 ─────────────────────────────────────────
    let state = AppState::new()
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("AppState::new 失败: {e}")))?;

    if let Err(e) = state.load_initial_session().await {
        warn!("load_initial_session 失败（未登录账号时属正常现象）: {e:#}");
    }

    let ctx = BootContext {
        state: Arc::new(state),
        cfg: Arc::new(RwLock::new(cfg)),
        _log_guard: log_guard,
    };

    // ── 4) 分发到 commands::*::run ─────────────────────────────────
    dispatch(&ctx, cli, printer).await
}

async fn dispatch(ctx: &BootContext, cli: Cli, printer: Printer) -> CliResult<()> {
    match cli.command {
        Commands::Login(args) => match args {
            cli::LoginCmd::Cookie { raw } => commands::login::cookie(ctx, &raw, &printer).await,
            cli::LoginCmd::Qrcode { poll_ms, timeout_s } => {
                commands::login::qrcode(ctx, poll_ms, timeout_s, &printer).await
            }
            cli::LoginCmd::Status { sign } => commands::login::qrcode_status(ctx, &sign, &printer).await,
            cli::LoginCmd::Logout => commands::login::logout(ctx, &printer).await,
        },

        Commands::Whoami => commands::login::whoami(ctx, &printer).await,

        Commands::Account(args) => match args {
            cli::AccountCmd::List => commands::account::list(ctx, &printer).await,
            cli::AccountCmd::Switch { uid } => commands::account::switch(ctx, uid, &printer).await,
            cli::AccountCmd::Delete { uid } => commands::account::delete(ctx, uid, &printer).await,
        },

        Commands::Ls(args) => commands::ls::run(ctx, args, &printer).await,
        Commands::Mkdir(args) => commands::file::mkdir(ctx, &args.path, &printer).await,
        Commands::Rm(args) => commands::file::rm(ctx, &args.paths, &printer).await,
        Commands::Mv(args) => commands::file::mv(ctx, &args.from, &args.to, args.name.as_deref(), &printer).await,
        Commands::Cp(args) => commands::file::cp(ctx, &args.from, &args.to, args.name.as_deref(), &printer).await,
        Commands::Rename(args) => commands::file::rename(ctx, &args.path, &args.newname, &printer).await,
        Commands::Search(args) => commands::file::search(ctx, &args.key, args.num, &printer).await,

        Commands::Upload(args) => commands::upload::file(ctx, args, &printer).await,
        Commands::UploadFolder(args) => commands::upload::folder(ctx, args, &printer).await,
        Commands::UploadBatch(args) => commands::upload::batch(ctx, args, &printer).await,

        Commands::Download(args) => commands::download::file(ctx, args, &printer).await,
        Commands::DownloadFolder(args) => commands::download::folder(ctx, args, &printer).await,

        Commands::Share(args) => match args {
            cli::ShareCmd::Preview { url, password } => {
                commands::share::preview(ctx, cli.account, &url, password.as_deref(), &printer).await
            }
            cli::ShareCmd::Transfer {
                url,
                password,
                save,
                auto_download,
                no_wait,
            } => {
                commands::share::transfer(
                    ctx,
                    cli.account,
                    &url,
                    password.as_deref(),
                    &save,
                    auto_download,
                    no_wait,
                    &printer,
                )
                .await
            }
        },

        Commands::Task(args) => match args {
            cli::TaskCmd::List { downloads, uploads, transfers } => {
                commands::task::list(ctx, cli.account, downloads, uploads, transfers, &printer).await
            }
            cli::TaskCmd::Status { id } => commands::task::status(ctx, &id, &printer).await,
            cli::TaskCmd::Pause { id } => commands::task::pause(ctx, &id, &printer).await,
            cli::TaskCmd::Resume { id } => commands::task::resume(ctx, &id, &printer).await,
            cli::TaskCmd::Cancel { id } => commands::task::cancel(ctx, &id, &printer).await,
            cli::TaskCmd::Wait { id, timeout_s } => {
                commands::task::wait(ctx, &id, timeout_s, &printer).await
            }
        },

        Commands::Config(args) => match args {
            cli::ConfigCmd::Show => commands::config::show(ctx, &cli.config, &printer).await,
            cli::ConfigCmd::Reload => commands::config::reload(&cli.config, &printer).await,
        },

        Commands::Ping => commands::login::ping(ctx, &printer).await,
    }
}
