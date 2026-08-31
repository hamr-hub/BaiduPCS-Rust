// login 子命令实现。
//
// 流程：
//   cookie <RAW>:
//     CookieLoginAuth::login_with_cookies → UserAuth
//     → AccountManager::add_user(upsert)
//     → set_active_persisted(Some(uid))
//
//   qrcode:
//     QRCodeAuth::generate_qrcode → 打印 sign + URL
//     循环 poll_status 直到 Success/Expired/Failed
//     Success → AccountManager::add_user + set_active_persisted
//
// 注意：登录成功后需要重新注入 per-uid manager
// （详见 backend/src/server/handlers/accounts.rs:switch_account）。
// 这里走 helpers 暴露的方法。

use std::time::Duration;

use baidu_netdisk_rust::auth::{CookieLoginAuth, LoginResponse, QRCodeAuth, QRCodeStatus, Uid, UserAuth};
use baidu_netdisk_rust::ProxyType;
use serde::Serialize;
use serde_json::json;

use crate::context::BootContext;
use crate::error::{CliError, CliResult};
use crate::output::Printer;

/// 健康检查
pub async fn ping(ctx: &BootContext, printer: &Printer) -> CliResult<()> {
    let (count, active_uid) = {
        let am = ctx.state.account_manager.lock().await;
        (am.list_users().len(), am.active_uid().map(|u| u.raw()))
    };

    let payload = json!({
        "ok": true,
        "accounts": count,
        "active_uid": active_uid,
        "readonly_mode": ctx.state.readonly_mode.load(std::sync::atomic::Ordering::Relaxed),
    });
    printer.result_json(&payload);
    Ok(())
}

/// whoami —— 当前活跃账号
pub async fn whoami(ctx: &BootContext, printer: &Printer) -> CliResult<()> {
    let user = ctx.active_user_auth().await?;
    let payload = json!({
        "uid": user.uid,
        "username": user.username,
        "nickname": user.nickname,
        "vip_type": user.vip_type,
        "total_space": user.total_space,
        "used_space": user.used_space,
        "avatar_url": user.avatar_url,
        "login_time": user.login_time,
        "last_warmup_at": user.last_warmup_at,
    });
    printer.result_json(&payload);
    Ok(())
}

/// Cookie 登录
pub async fn cookie(ctx: &BootContext, raw: &str, printer: &Printer) -> CliResult<()> {
    if raw.trim().is_empty() {
        return Err(CliError::BadArgument("Cookie 字符串为空".into()));
    }

    let cfg = ctx.cfg.read().await;
    let proxy = if cfg.network.proxy.proxy_type != ProxyType::None
        && !ctx.state.fallback_mgr.is_fallen_back()
    {
        Some(cfg.network.proxy.clone())
    } else {
        None
    };
    drop(cfg);

    let auth = CookieLoginAuth::new_with_proxy(proxy.as_ref())
        .map_err(|e| CliError::Core(anyhow::anyhow!("构造 CookieLoginAuth 失败: {e}")))?;

    let user = auth
        .login_with_cookies(raw)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("Cookie 登录失败: {e}")))?;

    persist_user(ctx, user.clone()).await?;

    printer.result_json(&json!({
        "ok": true,
        "user": user_summary(&user),
    }));
    printer.result_human(format!(
        "✓ 登录成功：uid={}, username={}",
        user.uid,
        user.username
    ));
    Ok(())
}

/// 二维码登录（生成 + 轮询）
pub async fn qrcode(
    ctx: &BootContext,
    poll_ms: u64,
    timeout_s: u64,
    printer: &Printer,
) -> CliResult<()> {
    let cfg = ctx.cfg.read().await;
    let proxy = if cfg.network.proxy.proxy_type != ProxyType::None
        && !ctx.state.fallback_mgr.is_fallen_back()
    {
        Some(cfg.network.proxy.clone())
    } else {
        None
    };
    drop(cfg);

    let auth = QRCodeAuth::new_with_proxy(proxy.as_ref())
        .map_err(|e| CliError::Core(anyhow::anyhow!("构造 QRCodeAuth 失败: {e}")))?;

    let qr = auth
        .generate_qrcode()
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("生成二维码失败: {e}")))?;

    printer.event("qrcode_generated", json!({
        "sign": qr.sign,
        "qrcode_url": qr.qrcode_url,
    }));
    printer.result_human(format!(
        "📱 请用百度网盘 App 扫码登录：{}\n   sign={}",
        qr.qrcode_url, qr.sign
    ));

    let start = std::time::Instant::now();
    loop {
        if timeout_s > 0 && start.elapsed().as_secs() > timeout_s {
            return Err(CliError::Core(anyhow::anyhow!("二维码登录超时")));
        }

        tokio::time::sleep(Duration::from_millis(poll_ms)).await;

        let status = auth
            .poll_status(&qr.sign)
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("轮询失败: {e}")))?;

        match status {
            QRCodeStatus::Waiting => {
                if !printer.quiet {
                    printer.progress(format!("等待扫码... elapsed={}s", start.elapsed().as_secs()));
                }
            }
            QRCodeStatus::Scanned => {
                printer.result_human("✓ 已扫码，请在手机上点击「确认登录」");
                if !printer.quiet {
                    printer.progress("等待手机确认...");
                }
            }
            QRCodeStatus::Success { user, token: _ } => {
                printer.progress_end();
                persist_user(ctx, user.clone()).await?;
                printer.result_json(&json!({"ok": true, "user": user_summary(&user)}));
                printer.result_human(format!(
                    "✓ 二维码登录成功：uid={}, username={}",
                    user.uid, user.username
                ));
                return Ok(());
            }
            QRCodeStatus::Expired => {
                return Err(CliError::Core(anyhow::anyhow!("二维码已过期，请重试")));
            }
            QRCodeStatus::Failed { reason } => {
                return Err(CliError::Core(anyhow::anyhow!("登录失败: {reason}")));
            }
        }
    }
}

/// 单次查询扫码状态
pub async fn qrcode_status(ctx: &BootContext, sign: &str, printer: &Printer) -> CliResult<()> {
    let cfg = ctx.cfg.read().await;
    let proxy = if cfg.network.proxy.proxy_type != ProxyType::None
        && !ctx.state.fallback_mgr.is_fallen_back()
    {
        Some(cfg.network.proxy.clone())
    } else {
        None
    };
    drop(cfg);

    let auth = QRCodeAuth::new_with_proxy(proxy.as_ref())
        .map_err(|e| CliError::Core(anyhow::anyhow!("构造 QRCodeAuth 失败: {e}")))?;
    let status = auth
        .poll_status(sign)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("轮询失败: {e}")))?;

    match status {
        QRCodeStatus::Waiting => printer.result_json(&json!({"status": "waiting"})),
        QRCodeStatus::Scanned => printer.result_json(&json!({"status": "scanned"})),
        QRCodeStatus::Success { user, token: _ } => {
            persist_user(ctx, user.clone()).await?;
            printer.result_json(&json!({"status": "success", "user": user_summary(&user)}));
        }
        QRCodeStatus::Expired => printer.result_json(&json!({"status": "expired"})),
        QRCodeStatus::Failed { reason } => {
            printer.result_json(&json!({"status": "failed", "reason": reason}))
        }
    }
    Ok(())
}

/// 退出当前活跃账号（不删除账号记录）
pub async fn logout(ctx: &BootContext, printer: &Printer) -> CliResult<()> {
    let mut am = ctx.state.account_manager.lock().await;
    am.set_active_persisted(None)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("set_active_persisted 失败: {e}")))?;
    *ctx.state.active_uid.write().await = None;
    drop(am);
    printer.result_json(&json!({"ok": true}));
    printer.result_human("✓ 已退出活跃账号");
    Ok(())
}

/// 把 UserAuth 写入 accounts.json + 切到活跃 + 构建 NetdiskClient + per-uid manager。
///
/// 与 web 服务器登录链路完全一致（参考
/// `backend/src/server/handlers/accounts.rs::cookie_login`）。
async fn persist_user(ctx: &BootContext, user: UserAuth) -> CliResult<()> {
    use std::sync::Arc;

    // 1) 写入 accounts.json（add_user 是 upsert）
    {
        let mut am = ctx.state.account_manager.lock().await;
        am.add_user(user.clone())
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("保存账号失败: {e}")))?;
        am.set_active_persisted(Some(Uid::new(user.uid)))
            .await
            .map_err(|e| CliError::Core(anyhow::anyhow!("设置活跃账号失败: {e}")))?;
    }
    *ctx.state.active_uid.write().await = Some(Uid::new(user.uid));

    // 2) 构造 NetdiskClient + 注入 client_pool
    let cfg = ctx.cfg.read().await;
    let proxy = if cfg.network.proxy.proxy_type != ProxyType::None
        && !ctx.state.fallback_mgr.is_fallen_back()
    {
        Some(cfg.network.proxy.clone())
    } else {
        None
    };
    drop(cfg);

    let client = baidu_netdisk_rust::netdisk::NetdiskClient::new_with_proxy(
        user.clone(),
        proxy.as_ref(),
        Some(Arc::clone(&ctx.state.fallback_mgr)),
    )
    .map_err(|e| CliError::Core(anyhow::anyhow!("构造 NetdiskClient 失败: {e}")))?;

    let client_arc = Arc::new(client);
    let uid = Uid::new(user.uid);
    ctx.state
        .client_pool
        .write()
        .await
        .add_client(uid, Arc::clone(&client_arc));

    // 3) 构造 per-uid DownloadManager / UploadManager / TransferManager
    //    与 web 服务端 `load_initial_session` 走同一构造路径。
    ctx.state
        .ensure_client_for_uid(uid)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("ensure_client_for_uid 失败: {e}")))?;

    let pm_arc = Arc::clone(&ctx.state.persistence_manager);
    ctx.state
        .build_and_register_managers_for_account(user.clone(), pm_arc)
        .await
        .map_err(|e| CliError::Core(anyhow::anyhow!("build_and_register_managers 失败: {e}")))?;

    Ok(())
}

#[derive(Debug, Serialize)]
struct UserSummary {
    uid: u64,
    username: String,
    nickname: Option<String>,
    vip_type: Option<u32>,
}

fn user_summary(u: &UserAuth) -> UserSummary {
    UserSummary {
        uid: u.uid,
        username: u.username.clone(),
        nickname: u.nickname.clone(),
        vip_type: u.vip_type,
    }
}

// 引用未使用但保留给将来扩展
#[allow(dead_code)]
fn _typecheck(_: &LoginResponse) {}
