// 上传子命令实现：file / folder / batch。
//
// 单文件走 UploadManager::create_task_with_owner；
// 文件夹走 create_folder_task（异步扫描）；
// 批量走 create_batch_tasks_with_owner。
//
// 默认行为：等待任务到终态；--no-wait 入队即返回。

use std::path::Path;

use baidu_netdisk_rust::{
    auth::Uid,
    uploader::conflict::UploadConflictStrategy,
};
use serde_json::json;

use crate::cli::{UploadArgs, UploadBatchArgs, UploadFolderArgs};
use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;
use crate::wait;

/// 单文件上传
pub async fn file(ctx: &BootContext, args: UploadArgs, printer: &Printer) -> CliResult<()> {
    if !args.local.exists() {
        return Err(CliError::LocalPathMissing(args.local.display().to_string()));
    }
    if !args.local.is_file() {
        return Err(CliError::BadArgument(format!(
            "{} 不是普通文件（请用 upload-folder 传目录）",
            args.local.display()
        )));
    }
    let uid = ctx.effective_uid(None).await?;
    let mgr = ctx.upload_manager_for(uid)?;

    let strategy: Option<UploadConflictStrategy> = args.conflict.map(Into::into);
    let task_id = mgr
        .create_task_with_owner(
            args.local.clone(),
            args.remote.clone(),
            args.encrypt,
            false,
            strategy,
            uid,
        )
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("create_task: {e}")))?;

    if task_id == "skipped" {
        printer.result_json(&json!({"ok": true, "skipped": true, "remote": args.remote}));
        printer.result_human(format!("⏭ 跳过（已存在）：{}", args.remote));
        return Ok(());
    }

    printer.event("task_enqueued", &json!({"kind": "upload", "id": task_id}));
    printer.result_json(&json!({"ok": true, "task_id": task_id, "remote": args.remote}));

    // 自动 start（create_task_with_owner 不会自动 start；handlers/upload.rs:222 会 start）
    if let Err(e) = mgr.start_task(&task_id).await {
        return Err(CliError::Core(anyhow::anyhow!("start_task: {e}")));
    }

    if args.no_wait {
        printer.result_human(format!("✓ 已入队：task_id={task_id}"));
        return Ok(());
    }

    let task = wait::for_upload(&mgr, &task_id, printer, args.timeout_s).await?;
    printer.result_json(&json!({
        "ok": true,
        "task_id": task.id,
        "remote": task.remote_path,
        "local": task.local_path,
        "size": task.total_size,
        "is_rapid_upload": task.is_rapid_upload,
    }));
    printer.result_human(format!(
        "✓ 上传完成：{} ({} → {})",
        task.remote_path,
        task.local_path.display(),
        crate::output::human_bytes(task.total_size)
    ));
    Ok(())
}

/// 文件夹上传（异步扫描）
pub async fn folder(
    ctx: &BootContext,
    args: UploadFolderArgs,
    printer: &Printer,
) -> CliResult<()> {
    if !args.local_dir.exists() || !args.local_dir.is_dir() {
        return Err(CliError::LocalPathMissing(args.local_dir.display().to_string()));
    }
    let uid = ctx.effective_uid(None).await?;
    let mgr = ctx.upload_manager_for(uid)?;

    let strategy: Option<UploadConflictStrategy> = args.conflict.map(Into::into);
    let task_ids = mgr
        .create_folder_task(
            args.local_dir.clone(),
            args.remote_dir.clone(),
            None,
            args.encrypt,
        )
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("create_folder_task: {e}")))?;

    let _ = strategy; // 留作未来传进 create_folder_task
    printer.result_json(&json!({
        "ok": true,
        "task_ids": task_ids,
        "remote_dir": args.remote_dir,
    }));

    if args.no_wait {
        printer.result_human(format!(
            "✓ 已入队 {} 个上传子任务",
            task_ids.len()
        ));
        return Ok(());
    }

    // 等所有子任务到终态
    let mut any_failed = false;
    for id in &task_ids {
        match wait::for_upload(&mgr, id, printer, args.timeout_s).await {
            Ok(_) => {}
            Err(CliError::TaskFailed(msg)) => {
                printer.result_human(format!("✗ 子任务 {id} 失败：{msg}"));
                any_failed = true;
            }
            Err(e) => return Err(e),
        }
    }

    if any_failed {
        return Err(CliError::TaskFailed("部分子任务失败".into()));
    }
    printer.result_human("✓ 文件夹上传完成");
    Ok(())
}

/// 批量上传：LOCAL=REMOTE 格式
pub async fn batch(
    ctx: &BootContext,
    args: UploadBatchArgs,
    printer: &Printer,
) -> CliResult<()> {
    let pairs = parse_pairs(&args.items)?;
    for (local, _) in &pairs {
        if !local.exists() {
            return Err(CliError::LocalPathMissing(local.display().to_string()));
        }
    }
    let uid = ctx.effective_uid(None).await?;
    let mgr = ctx.upload_manager_for(uid)?;

    let strategy: Option<UploadConflictStrategy> = args.conflict.map(Into::into);
    let task_ids = mgr
        .create_batch_tasks_with_owner(pairs.clone(), args.encrypt, strategy, uid)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("create_batch_tasks: {e}")))?;

    printer.result_json(&json!({
        "ok": true,
        "task_ids": task_ids,
        "count": task_ids.len(),
    }));

    if args.no_wait {
        printer.result_human(format!("✓ 已入队 {} 个任务", task_ids.len()));
        return Ok(());
    }

    for id in &task_ids {
        if id == "skipped" {
            continue;
        }
        if let Err(e) = wait::for_upload(&mgr, id, printer, 0).await {
            printer.result_human(format!("✗ {id}：{e}"));
        }
        // 启动失败的 task
        let _ = mgr.start_task(id).await;
    }

    printer.result_human("✓ 批量上传完成");
    Ok(())
}

fn parse_pairs(items: &[String]) -> CliResult<Vec<(std::path::PathBuf, String)>> {
    items
        .iter()
        .map(|s| {
            let (l, r) = s
                .split_once('=')
                .ok_or_else(|| CliError::BadArgument(format!("格式错误（应为 LOCAL=REMOTE）：{s}")))?;
            Ok::<_, CliError>((Path::new(l).to_path_buf(), r.to_string()))
        })
        .collect()
}

#[allow(dead_code)]
fn _uid_helper(u: Uid) -> u64 {
    u.raw()
}
