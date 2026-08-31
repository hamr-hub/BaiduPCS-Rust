// 任务管理子命令：list / status / pause / resume / cancel / wait。
//
// list 默认列三类；用 --downloads/--uploads/--transfers 过滤。
// status/pause/resume/cancel/wait 通过 task_id 反查所属 manager（跨账号）。

use serde_json::json;

use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;
use crate::wait;

pub async fn list(
    ctx: &BootContext,
    override_uid: Option<u64>,
    downloads: bool,
    uploads: bool,
    transfers: bool,
    printer: &Printer,
) -> CliResult<()> {
    let all = !downloads && !uploads && !transfers;
    let uid = ctx.effective_uid(override_uid).await.ok();
    let mut out = serde_json::Map::new();

    if all || downloads {
        if let Some(uid) = uid {
            if let Some(mgr) = ctx.state.download_manager_for(uid) {
                let tasks = mgr
                    .get_all_tasks()
                    .await;
                out.insert(
                    "downloads".into(),
                    json!(tasks.iter().map(download_to_json).collect::<Vec<_>>()),
                );
            }
        }
    }
    if all || uploads {
        if let Some(uid) = uid {
            if let Some(mgr) = ctx.state.upload_manager_for(uid) {
                let tasks = mgr.get_all_tasks().await;
                out.insert(
                    "uploads".into(),
                    json!(tasks.iter().map(upload_to_json).collect::<Vec<_>>()),
                );
            }
        }
    }
    if all || transfers {
        if let Some(uid) = uid {
            if let Some(mgr) = ctx.state.transfer_manager_for(uid) {
                let tasks = mgr
                    .get_live_tasks()
                    .await;
                out.insert(
                    "transfers".into(),
                    json!(tasks.iter().map(transfer_to_json).collect::<Vec<_>>()),
                );
            }
        }
    }

    printer.result_json(&serde_json::Value::Object(out));
    Ok(())
}

pub async fn status(ctx: &BootContext, id: &str, printer: &Printer) -> CliResult<()> {
    // 尝试三类 manager
    if let Ok((uid, mgr)) = ctx.find_download_manager(id).await {
        if let Some(t) = mgr.get_task(id).await {
            let mut obj = download_to_json(&t);
            obj["owner_uid"] = json!(uid.raw());
            printer.result_json(&obj);
            return Ok(());
        }
    }
    if let Ok((uid, mgr)) = ctx.find_upload_manager(id).await {
        if let Some(t) = mgr.get_task(id).await {
            let mut obj = upload_to_json(&t);
            obj["owner_uid"] = json!(uid.raw());
            printer.result_json(&obj);
            return Ok(());
        }
    }
    if let Ok((uid, mgr)) = ctx.find_transfer_manager(id).await {
        if let Some(t) = mgr.get_task(id).await {
            let mut obj = transfer_to_json(&t);
            obj["owner_uid"] = json!(uid.raw());
            printer.result_json(&obj);
            return Ok(());
        }
    }
    Err(CliError::TaskNotFound(id.to_string()))
}

pub async fn pause(ctx: &BootContext, id: &str, printer: &Printer) -> CliResult<()> {
    if let Ok((_, mgr)) = ctx.find_download_manager(id).await {
        mgr.pause_task(id, false)
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("pause: {e}")))?;
        printer.result_json(&json!({"ok": true, "id": id, "op": "pause"}));
        return Ok(());
    }
    if let Ok((_, mgr)) = ctx.find_upload_manager(id).await {
        mgr.pause_task(id, false)
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("pause: {e}")))?;
        printer.result_json(&json!({"ok": true, "id": id, "op": "pause"}));
        return Ok(());
    }
    Err(CliError::TaskNotFound(id.to_string()))
}

pub async fn resume(ctx: &BootContext, id: &str, printer: &Printer) -> CliResult<()> {
    if let Ok((_, mgr)) = ctx.find_download_manager(id).await {
        mgr.resume_task(id)
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("resume: {e}")))?;
        printer.result_json(&json!({"ok": true, "id": id, "op": "resume"}));
        return Ok(());
    }
    if let Ok((_, mgr)) = ctx.find_upload_manager(id).await {
        mgr.resume_task(id)
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("resume: {e}")))?;
        printer.result_json(&json!({"ok": true, "id": id, "op": "resume"}));
        return Ok(());
    }
    Err(CliError::TaskNotFound(id.to_string()))
}

pub async fn cancel(ctx: &BootContext, id: &str, printer: &Printer) -> CliResult<()> {
    // 下载：cancel_task_without_delete（删除 task 但保留本地文件）
    if let Ok((_, mgr)) = ctx.find_download_manager(id).await {
        mgr.cancel_task_without_delete(id);
        printer.result_json(&json!({"ok": true, "id": id, "op": "cancel"}));
        return Ok(());
    }
    if let Ok((_, mgr)) = ctx.find_upload_manager(id).await {
        mgr.cancel_task(id)
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("cancel: {e}")))?;
        printer.result_json(&json!({"ok": true, "id": id, "op": "cancel"}));
        return Ok(());
    }
    if let Ok((_, mgr)) = ctx.find_transfer_manager(id).await {
        mgr.cancel_task(id)
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("cancel: {e}")))?;
        printer.result_json(&json!({"ok": true, "id": id, "op": "cancel"}));
        return Ok(());
    }
    Err(CliError::TaskNotFound(id.to_string()))
}

pub async fn wait(
    ctx: &BootContext,
    id: &str,
    timeout_s: u64,
    printer: &Printer,
) -> CliResult<()> {
    if let Ok((_, mgr)) = ctx.find_download_manager(id).await {
        let t = wait::for_download(&mgr, id, printer, timeout_s).await?;
        printer.result_json(&download_to_json(&t));
        return Ok(());
    }
    if let Ok((_, mgr)) = ctx.find_upload_manager(id).await {
        let t = wait::for_upload(&mgr, id, printer, timeout_s).await?;
        printer.result_json(&upload_to_json(&t));
        return Ok(());
    }
    if let Ok((_, mgr)) = ctx.find_transfer_manager(id).await {
        let t = wait::for_transfer(&mgr, id, printer, timeout_s).await?;
        printer.result_json(&transfer_to_json(&t));
        return Ok(());
    }
    Err(CliError::TaskNotFound(id.to_string()))
}

// ── 转换到 JSON ──────────────────────────────────────────────────────

fn download_to_json(t: &baidu_netdisk_rust::DownloadTask) -> serde_json::Value {
    json!({
        "id": t.id,
        "kind": "download",
        "remote_path": t.remote_path,
        "local_path": t.local_path,
        "fs_id": t.fs_id,
        "total_size": t.total_size,
        "downloaded_size": t.downloaded_size,
        "status": format!("{:?}", t.status),
        "speed": t.speed,
        "error": t.error,
        "is_encrypted": t.is_encrypted,
    })
}

fn upload_to_json(t: &baidu_netdisk_rust::UploadTask) -> serde_json::Value {
    json!({
        "id": t.id,
        "kind": "upload",
        "remote_path": t.remote_path,
        "local_path": t.local_path,
        "total_size": t.total_size,
        "uploaded_size": t.uploaded_size,
        "status": format!("{:?}", t.status),
        "speed": t.speed,
        "is_rapid_upload": t.is_rapid_upload,
        "encrypt_enabled": t.encrypt_enabled,
        "error": t.error,
        "failure_reason": t.failure_reason,
    })
}

fn transfer_to_json(t: &baidu_netdisk_rust::TransferTask) -> serde_json::Value {
    json!({
        "id": t.id,
        "kind": "transfer",
        "share_url": t.share_url,
        "save_path": t.save_path,
        "status": format!("{:?}", t.status),
        "transferred_count": t.transferred_count,
        "total_count": t.total_count,
        "auto_download": t.auto_download,
        "error": t.error,
        "download_task_ids": t.download_task_ids,
    })
}
