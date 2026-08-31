// 分享转存子命令：preview / transfer。
//
// preview: TransferManager::preview_share → 列出分享的文件
// transfer: TransferManager::create_task → 异步转存到目标目录
//           可选 auto_download，转存后自动下到本地

use std::path::PathBuf;

use baidu_netdisk_rust::transfer::manager::CreateTransferRequest;
use serde_json::json;

use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;
use crate::wait;

pub async fn preview(
    ctx: &BootContext,
    override_uid: Option<u64>,
    url: &str,
    password: Option<&str>,
    printer: &Printer,
) -> CliResult<()> {
    let uid = ctx.effective_uid(override_uid).await?;
    let mgr = ctx.transfer_manager_for(uid)?;
    let preview = mgr
        .preview_share(url, password.map(|s| s.to_string()), 1, 100)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("preview_share: {e}")))?;

    let items: Vec<_> = preview
        .files
        .iter()
        .map(|f| {
            json!({
                "fs_id": f.fs_id,
                "path": f.path,
                "filename": f.name,
                "size": f.size,
                "isdir": f.is_dir,
            })
        })
        .collect();

    printer.result_json(&json!({
        "url": url,
        "need_password": password.is_none() && items.is_empty(),
        "short_key": preview.short_key,
        "share_root": preview.share_root_path,
        "items": items,
        "count": items.len(),
    }));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn transfer(
    ctx: &BootContext,
    override_uid: Option<u64>,
    url: &str,
    password: Option<&str>,
    save: &str,
    auto_download: bool,
    no_wait: bool,
    printer: &Printer,
) -> CliResult<()> {
    let uid = ctx.effective_uid(override_uid).await?;
    let client = ctx.client_for(uid).await?;
    let mgr = ctx.transfer_manager_for(uid)?;

    // save 是网盘目标目录，需要 save_fs_id
    let parent = save.trim_end_matches('/');
    let parent = if parent.is_empty() { "/" } else { parent };
    let list = client
        .get_file_list(parent, 1, 1)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("get_file_list({parent}): {e}")))?;
    let save_fs_id = list.list.first().map(|f| f.fs_id).unwrap_or(0);

    let req = CreateTransferRequest {
        share_url: url.to_string(),
        password: password.map(|s| s.to_string()),
        randsk: None,
        save_path: save.to_string(),
        save_fs_id,
        auto_download: Some(auto_download),
        local_download_path: None,
        download_conflict_strategy: None,
        is_share_direct_download: false,
        selected_fs_ids: None,
        selected_files: None,
        owner_uid_override: None,
        is_internal: false,
        backup_config_id: None,
        prefetched_share: None,
    };

    let resp = mgr
        .create_task(req)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("create_task: {e}")))?;

    if resp.need_password {
        return Err(CliError::BadArgument("分享需要密码，请用 --password".into()));
    }
    let task_id = resp.task_id.ok_or_else(|| {
        CliError::Core(anyhow::anyhow!(
            "create_task 返回 None：{:?}",
            resp.error
        ))
    })?;

    printer.event("task_enqueued", &json!({"kind": "share_transfer", "id": task_id}));
    printer.result_json(&json!({
        "ok": true,
        "task_id": task_id,
        "save_path": save,
    }));

    if no_wait {
        printer.result_human(format!("✓ 已入队：task_id={task_id}"));
        return Ok(());
    }

    let task = wait::for_transfer(&mgr, &task_id, printer, 0).await?;
    printer.result_json(&json!({
        "ok": true,
        "task_id": task.id,
        "save_path": task.save_path,
        "transferred_count": task.transferred_count,
        "total_count": task.total_count,
    }));
    printer.result_human(format!(
        "✓ 转存完成：{} ({}/{})",
        task.save_path, task.transferred_count, task.total_count
    ));

    if auto_download {
        // 等自动下载子任务完成（简化：取 download_task_ids 里最后一个等终态）
        printer.result_human("⏳ 等待自动下载子任务...");
        for sub_id in &task.download_task_ids {
            let (owner_uid, dm) = ctx
                .find_download_manager(sub_id)
                .await
                .unwrap_or((uid, ctx.download_manager_for(uid)?));
            let _ = owner_uid;
            if let Ok(_) = wait::for_download(&dm, sub_id, printer, 0).await {
                // 成功；继续
            }
        }
    }

    Ok(())
}

// 引用保持（PathBuf 来自 std）
#[allow(dead_code)]
fn _ensure_path_used(_: PathBuf) {}
