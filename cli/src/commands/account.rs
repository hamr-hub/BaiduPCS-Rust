// 账号管理子命令实现。
//
// list    - AccountManager::list_accounts（返回 AccountSummary，不含敏感字段）
// switch  - AccountManager::set_active_persisted(Some(uid))
// delete  - AccountManager::delete_user(uid)

use baidu_netdisk_rust::auth::Uid;
use serde_json::json;

use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;

pub async fn list(ctx: &BootContext, printer: &Printer) -> CliResult<()> {
    let (active_uid, accounts): (Option<u64>, Vec<_>) = {
        let am = ctx.state.account_manager.lock().await;
        let active = am.active_uid().map(|u| u.raw());
        let accs: Vec<_> = am
            .list_accounts()
            .into_iter()
            .map(|s| {
                json!({
                    "uid": s.uid,
                    "username": s.username,
                    "nickname": s.nickname,
                    "vip_type": s.vip_type,
                    "is_active": Some(s.uid) == active,
                    "has_custom_config": s.custom_config.auto_apply_recommended,
                })
            })
            .collect();
        (active, accs)
    };

    printer.result_json(&json!({
        "accounts": accounts,
        "active_uid": active_uid,
    }));
    Ok(())
}

pub async fn switch(ctx: &BootContext, uid: u64, printer: &Printer) -> CliResult<()> {
    let target = Uid::new(uid);
    {
        let mut am = ctx.state.account_manager.lock().await;
        if am.get_user(target).is_none() {
            return Err(CliError::UnknownAccount(uid));
        }
        am.set_active_persisted(Some(target))
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("set_active_persisted: {e}")))?;
    }
    *ctx.state.active_uid.write().await = Some(target);

    // 切账号后必须确保 client 已注入 client_pool（其它账号的 client 可能未预热）
    ctx.state
        .ensure_client_for_uid(target)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("ensure_client_for_uid: {e}")))?;

    printer.result_json(&json!({"ok": true, "active_uid": uid}));
    printer.result_human(format!("✓ 已切换到 uid={uid}"));
    Ok(())
}

pub async fn delete(ctx: &BootContext, uid: u64, printer: &Printer) -> CliResult<()> {
    let target = Uid::new(uid);
    {
        let mut am = ctx.state.account_manager.lock().await;
        if am.get_user(target).is_none() {
            return Err(CliError::UnknownAccount(uid));
        }
        am.delete_user(target)
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("delete_user: {e}")))?;
    }

    // 清掉 client_pool 里的对应客户端
    {
        let mut pool = ctx.state.client_pool.write().await;
        pool.remove_client(target);
    }
    // 清掉 per-uid manager
    ctx.state.download_managers.remove(&target);
    ctx.state.upload_managers.remove(&target);
    ctx.state.transfer_managers.remove(&target);

    // 如果删的就是活跃账号，active_uid 重置
    if *ctx.state.active_uid.read().await == Some(target) {
        let am = ctx.state.account_manager.lock().await;
        *ctx.state.active_uid.write().await = am.active_uid();
    }

    printer.result_json(&json!({"ok": true, "deleted_uid": uid}));
    printer.result_human(format!("✓ 已删除账号 uid={uid}"));
    Ok(())
}
