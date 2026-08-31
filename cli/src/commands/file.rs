// 文件 / 目录管理子命令：mkdir / rm / mv / cp / rename / search。
//
// 所有命令直接调用 NetdiskClient 上的方法。

use baidu_netdisk_rust::netdisk::{FileOperationItem, RenameItem};
use serde_json::json;

use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;

pub async fn mkdir(ctx: &BootContext, path: &str, printer: &Printer) -> CliResult<()> {
    let client = ctx.active_client().await?;
    let resp = client
        .create_folder(path)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("create_folder: {e}")))?;
    printer.result_json(&json!({
        "ok": true,
        "path": path,
        "fs_id": resp.fs_id,
        "errno": resp.errno,
    }));
    printer.result_human(format!("✓ 已创建目录：{path}"));
    Ok(())
}

pub async fn rm(ctx: &BootContext, paths: &[String], printer: &Printer) -> CliResult<()> {
    let client = ctx.active_client().await?;
    let resp = client
        .delete_files_chunked(paths)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("delete_files_chunked: {e}")))?;
    printer.result_json(&json!({
        "ok": resp.success,
        "errno": resp.errno,
        "error": resp.error,
        "failed_paths": resp.failed_paths,
        "deleted_count": resp.deleted_count,
        "requested": paths,
    }));
    if resp.success {
        printer.result_human(format!("✓ 已删除 {} 个路径", resp.deleted_count));
    } else {
        printer.result_human(format!(
            "✗ 删除部分失败（成功 {}，失败 {}）",
            resp.deleted_count,
            resp.failed_paths.len()
        ));
    }
    Ok(())
}

pub async fn mv(
    ctx: &BootContext,
    from: &str,
    to: &str,
    newname: Option<&str>,
    printer: &Printer,
) -> CliResult<()> {
    let client = ctx.active_client().await?;
    let newname_owned = match newname {
        Some(n) => n.to_string(),
        None => extract_filename(from)?,
    };
    let item = FileOperationItem {
        path: from.to_string(),
        dest: to.to_string(),
        newname: newname_owned,
    };
    let outcome = client
        .move_files(&[item])
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("move_files: {e}")))?;
    report_outcome(&outcome, "move", printer);
    Ok(())
}

pub async fn cp(
    ctx: &BootContext,
    from: &str,
    to: &str,
    newname: Option<&str>,
    printer: &Printer,
) -> CliResult<()> {
    let client = ctx.active_client().await?;
    let newname_owned = match newname {
        Some(n) => n.to_string(),
        None => extract_filename(from)?,
    };
    let item = FileOperationItem {
        path: from.to_string(),
        dest: to.to_string(),
        newname: newname_owned,
    };
    let outcome = client
        .copy_files(&[item])
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("copy_files: {e}")))?;
    report_outcome(&outcome, "copy", printer);
    Ok(())
}

pub async fn rename(
    ctx: &BootContext,
    path: &str,
    newname: &str,
    printer: &Printer,
) -> CliResult<()> {
    let client = ctx.active_client().await?;

    // rename 需要 fs_id，先查 parent 拿一下
    let parent = parent_dir(path);
    let filename = extract_filename(path)?;
    let list = client
        .get_file_list(&parent, 1, 200)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("get_file_list: {e}")))?;
    let item = list
        .list
        .iter()
        .find(|f| f.server_filename == filename)
        .ok_or_else(|| CliError::BadArgument(format!("找不到文件 {path}")))?;

    let rename_item = RenameItem {
        path: path.to_string(),
        newname: newname.to_string(),
        id: item.fs_id,
    };
    let outcome = client
        .rename_file(rename_item)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("rename_file: {e}")))?;
    report_outcome(&outcome, "rename", printer);
    Ok(())
}

pub async fn search(
    ctx: &BootContext,
    key: &str,
    num: u32,
    printer: &Printer,
) -> CliResult<()> {
    let client = ctx.active_client().await?;
    let resp = client
        .search_files(key, 1, num, 1)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("search_files: {e}")))?;
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
            })
        })
        .collect();
    printer.result_json(&json!({"key": key, "items": items}));
    Ok(())
}

// ── helpers ────────────────────────────────────────────────────────────

fn extract_filename(path: &str) -> CliResult<String> {
    let p = std::path::Path::new(path);
    match p.file_name() {
        Some(name) => Ok(name.to_string_lossy().into_owned()),
        None => Err(CliError::BadArgument(format!("无法从 {path} 提取文件名"))),
    }
}

fn parent_dir(path: &str) -> String {
    let p = std::path::Path::new(path);
    match p.parent() {
        Some(par) if !par.as_os_str().is_empty() => {
            let s = par.to_string_lossy().into_owned();
            if s.starts_with('/') {
                s.trim_end_matches('/').to_string()
            } else {
                format!("/{}", s.trim_end_matches('/'))
            }
        }
        _ => "/".to_string(),
    }
}

fn report_outcome(
    outcome: &baidu_netdisk_rust::netdisk::FileOperationOutcome,
    op: &str,
    printer: &Printer,
) {
    use baidu_netdisk_rust::netdisk::FileOperationOutcome::*;
    match outcome {
        Success(s) => {
            printer.result_json(&json!({
                "ok": true,
                "op": op,
                "taskid": s.taskid,
                "total": s.total,
            }));
            printer.result_human(format!("✓ {op} 任务已提交，taskid={}", s.taskid));
        }
        Failed { message, payload } => {
            printer.result_json(&json!({
                "ok": false,
                "op": op,
                "message": message,
                "payload": payload,
            }));
            printer.result_human(format!("✗ {op} 失败：{message}"));
        }
    }
}
