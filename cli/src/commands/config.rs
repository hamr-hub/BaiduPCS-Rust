// 配置管理子命令：show / reload。
//
// 简化实现：仅打印当前内存中的 AppConfig；reload 走 AppConfig::load_from_file。

use std::path::PathBuf;

use baidu_netdisk_rust::AppConfig;
use serde_json::json;

use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;

pub async fn show(
    ctx: &BootContext,
    override_path: &Option<PathBuf>,
    printer: &Printer,
) -> CliResult<()> {
    let cfg = ctx.cfg.read().await;
    printer.result_json(&json!({
        "config_path": override_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "config/app.toml".into()),
        "server": &cfg.server,
        "download": &cfg.download,
        "upload": &cfg.upload,
        "transfer": &cfg.transfer,
        "filesystem": &cfg.filesystem,
        "persistence": &cfg.persistence,
        "log": &cfg.log,
        "network": &cfg.network,
        "scan": &cfg.scan,
        "conflict_strategy": &cfg.conflict_strategy,
    }));
    Ok(())
}

pub async fn reload(override_path: &Option<PathBuf>, printer: &Printer) -> CliResult<()> {
    let path = override_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config/app.toml".to_string());
    let cfg = AppConfig::load_from_file(&path)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("reload: {e}")))?;
    printer.result_json(&json!({"ok": true, "config_path": path}));
    printer.result_human(format!("✓ 配置已重新加载：{path}"));
    // 注意：reload 仅验证 TOML 解析，不会替换 AppState.config（要替换需要更深入的 wiring；
    // 此处提供的是「下次启动生效」的语义，与大多数 CLI 一致）
    printer.result_human("ℹ  提示：AppState 内的配置仍为启动时加载的版本；变更在重启 CLI 后生效");
    let _ = cfg;
    Ok(())
}
