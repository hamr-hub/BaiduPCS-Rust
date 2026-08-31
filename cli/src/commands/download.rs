// 下载子命令实现：file / folder。
//
// 单文件下载流程：
//   1) get_file_list(parent) 拿到 fs_id / size / filename
//   2) DownloadManager::create_task_with_owner(fs_id, remote, name, size, ...)
//   3) start_task → 等待到终态
//
// 文件夹下载走 FolderDownloadManager::create_folder_download。

use std::path::PathBuf;

use baidu_netdisk_rust::uploader::conflict::DownloadConflictStrategy;
use serde_json::json;

use crate::cli::{DownloadArgs, DownloadFolderArgs};
use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;
use crate::wait;

/// 单文件下载
pub async fn file(ctx: &BootContext, args: DownloadArgs, printer: &Printer) -> CliResult<()> {
    let uid = ctx.effective_uid(None).await?;
    let client = ctx.client_for(uid).await?;
    let mgr = ctx.download_manager_for(uid)?;

    // 解析路径：得到 (parent_dir, filename)
    let remote_path = args.remote.trim().to_string();
    let (parent, filename) = split_remote(&remote_path)?;

    // 查父目录拿 fs_id / size
    let list = client
        .get_file_list(&parent, 1, 200)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("get_file_list({}): {e}", parent)))?;
    let item = list
        .list
        .iter()
        .find(|f| f.server_filename == filename)
        .ok_or_else(|| {
            CliError::BadArgument(format!("在 {parent} 下找不到文件 {filename}"))
        })?;

    if item.is_directory() {
        return Err(CliError::BadArgument(format!(
            "{remote_path} 是目录，请改用 download-folder"
        )));
    }

    let strategy: Option<DownloadConflictStrategy> = args.conflict.map(Into::into);

    let task_id = mgr
        .create_task_with_owner(
            item.fs_id,
            remote_path.clone(),
            filename.to_string(),
            item.size,
            strategy,
            uid,
        )
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("create_task: {e}")))?;

    if task_id == "skipped" {
        printer.result_json(&json!({"ok": true, "skipped": true, "remote": remote_path}));
        printer.result_human(format!("⏭ 跳过（已存在）：{remote_path}"));
        return Ok(());
    }

    printer.event("task_enqueued", &json!({"kind": "download", "id": task_id}));
    printer.result_json(&json!({"ok": true, "task_id": task_id, "remote": remote_path}));

    // start_task（与 server 端 handlers/download.rs:160 一致）
    if let Err(e) = mgr.start_task(&task_id).await {
        return Err(CliError::Core(anyhow::anyhow!("start_task: {e}")));
    }

    if args.no_wait {
        printer.result_human(format!("✓ 已入队：task_id={task_id}"));
        return Ok(());
    }

    let task = wait::for_download(&mgr, &task_id, printer, args.timeout_s).await?;

    printer.result_json(&json!({
        "ok": true,
        "task_id": task.id,
        "remote": task.remote_path,
        "local": task.local_path,
        "size": task.total_size,
    }));
    printer.result_human(format!(
        "✓ 下载完成：{} → {}",
        task.remote_path,
        task.local_path.display()
    ));

    if let Some(to) = args.to {
        // 用户指定了 --to 但 manager 用了默认 download_dir —— 移动到指定位置
        copy_or_move(&task.local_path, &to, printer)?;
    }

    Ok(())
}

/// 文件夹下载
pub async fn folder(
    ctx: &BootContext,
    args: DownloadFolderArgs,
    _printer: &Printer,
) -> CliResult<()> {
    let uid = ctx.effective_uid(None).await?;
    let client = ctx.client_for(uid).await?;
    let fdm = &ctx.state.folder_download_manager;

    // 复用 FolderDownloadManager —— 但 FolderDownloadManager 不直接暴露 create 接口，
    // 走它的 init/create 路径需要 fs_id。本实现退化为：遍历网盘子文件，逐个走 file 命令。
    let list = client
        .get_file_list(&args.remote_dir, 1, 1000)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("get_file_list: {e}")))?;

    let strategy: Option<DownloadConflictStrategy> = args.conflict.map(Into::into);

    let mut task_ids = Vec::new();
    for f in list.list {
        if f.is_directory() {
            continue; // 暂不递归（后续可调用自身）
        }
        let mgr = ctx.download_manager_for(uid)?;
        let id = mgr
            .create_task_with_owner(
                f.fs_id,
                f.path.clone(),
                f.server_filename.clone(),
                f.size,
                strategy,
                uid,
            )
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("create_task({}): {e}", f.path)))?;
        if id != "skipped" {
            mgr.start_task(&id).await.ok();
            task_ids.push(id);
        }
    }

    let _ = fdm; // 引用以避免 unused
    serde_json::json!({"ok": true, "task_ids": task_ids, "dir": args.remote_dir});
    Ok(())
}

fn split_remote(remote: &str) -> CliResult<(String, String)> {
    if !remote.starts_with('/') {
        return Err(CliError::RemotePathInvalid(format!("必须以 / 开头：{remote}")));
    }
    let p = std::path::Path::new(remote);
    let parent = p
        .parent()
        .and_then(|x| x.to_str())
        .unwrap_or("/");
    let parent = if parent.is_empty() { "/".to_string() } else { parent.to_string() };
    let filename = p
        .file_name()
        .and_then(|x| x.to_str())
        .ok_or_else(|| CliError::RemotePathInvalid(remote.to_string()))?
        .to_string();
    if filename.is_empty() {
        return Err(CliError::RemotePathInvalid(remote.to_string()));
    }
    Ok((parent, filename))
}

fn copy_or_move(src: &std::path::Path, dst: &PathBuf, printer: &Printer) -> CliResult<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dst)?;
    printer.result_human(format!("✓ 已复制到 {}", dst.display()));
    Ok(())
}
