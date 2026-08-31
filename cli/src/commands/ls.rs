// ls 子命令：列出网盘目录。
//
// 走 NetdiskClient::get_file_list，支持分页。

use std::path::Path;

use serde_json::json;

use crate::cli::LsArgs;
use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;

pub async fn run(ctx: &BootContext, args: LsArgs, printer: &Printer) -> CliResult<()> {
    let client = ctx.active_client().await?;
    let path = normalize_remote(&args.path)?;
    let resp = client
        .get_file_list(&path, args.page, args.page_size)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("get_file_list: {e}")))?;

    if printer.mode == crate::output::OutputMode::Human {
        // 人类模式：表格
        printer.result_human(format!(
            "📂 {} (errno={}, page={}/{})",
            path, resp.errno, args.page, args.page_size
        ));
        if resp.list.is_empty() {
            printer.result_human("（空目录）");
        } else {
            printer.result_human(format!(
                "{:<60}  {:>10}  {:<24}  {}",
                "名称", "大小", "修改时间", "类型"
            ));
            printer.result_human("-".repeat(120));
            for f in &resp.list {
                let size = if f.is_directory() {
                    "-".to_string()
                } else {
                    crate::output::human_bytes(f.size)
                };
                let mtime = chrono::DateTime::from_timestamp(f.server_mtime, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| f.server_mtime.to_string());
                let kind = if f.is_directory() { "目录" } else { "文件" };
                printer.result_human(format!(
                    "{:<60}  {:>10}  {:<24}  {}",
                    truncate(&f.server_filename, 60),
                    size,
                    mtime,
                    kind
                ));
            }
        }
    } else {
        // JSON 模式：完整结构
        let items: Vec<_> = resp
            .list
            .iter()
            .map(|f| {
                json!({
                    "fs_id": f.fs_id,
                    "path": f.path,
                    "filename": f.server_filename,
                    "size": f.size,
                    "isdir": f.is_directory(),
                    "category": f.category,
                    "server_mtime": f.server_mtime,
                    "md5": f.md5,
                })
            })
            .collect();
        printer.result_json(&json!({
            "path": path,
            "errno": resp.errno,
            "page": args.page,
            "page_size": args.page_size,
            "items": items,
        }));
    }
    Ok(())
}

fn normalize_remote(p: &str) -> CliResult<String> {
    if p.is_empty() {
        return Err(CliError::BadArgument("path 不能为空".into()));
    }
    let path = Path::new(p);
    let s = path.to_string_lossy();
    let s = if s.starts_with('/') {
        s.into_owned()
    } else {
        format!("/{}", s.trim_start_matches('/'))
    };
    Ok(s.trim_end_matches('/').to_string())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_remote() {
        assert_eq!(normalize_remote("/foo").unwrap(), "/foo");
        assert_eq!(normalize_remote("foo").unwrap(), "/foo");
        assert_eq!(normalize_remote("/foo/").unwrap(), "/foo");
        assert_eq!(normalize_remote("").is_err(), true);
    }
}
