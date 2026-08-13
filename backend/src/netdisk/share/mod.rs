//! 分享链接策略层
//!
//! 百度网盘有两套互不兼容的分享体系：
//!
//! | | 个人版 | 企业版（apaas） |
//! |---|---|---|
//! | 链接 | `pan.baidu.com/s/1xxxx` | `pan.baidu.com/apaas/share?surl=xxxx` |
//! | 短链 | `surl` 要补 `1` 前缀 | `surl` 原样，**不能补前缀** |
//! | 分享页 | HTML 里刨 `yunData` 片段 | `window.yunData = {...}` 干净 JSON |
//! | 验证提取码 | `/share/verify` → `randsk` | `/apaas/api/share/getspwd` → `spwd` |
//! | 列目录 | `/share/list`（shareid+uk+bdstoken） | `/apaas/1.0/share/list`（short_url+spwd） |
//! | 转存 | `/share/transfer` | `/apaas/1.0/share/transfer` |
//!
//! 拿企业版链接去调个人版接口，`/s/1<surl>` 会命中一个「链接不存在」的错误页，
//! 而该错误页里仍嵌着无关的 `shareid`，于是解析「成功」、后续 `/share/verify`
//! 稳定返回 `errno=-12`——这就是 issue #139 的现象。
//!
//! 上层（`transfer::manager`、`share_sync`）的编排逻辑对两者完全一致，
//! 所以这里用策略模式把差异收敛到 [`ShareProvider`] 的 6 个方法，
//! 公共部分（文件项解析、轮询节奏、结果提取）放 [`common`] 复用。
//!
//! 一个关键复用：企业版的 `spwd` 与个人版的 `randsk` 语义相同（都是校验提取码
//! 后换来的、后续请求要带的凭据），因此它直接复用现有的 `randsk` 通道
//! （`TransferTask::randsk` → 持久化 → `*_with_randsk`），上层无需感知差异。

pub mod apaas;
pub mod common;
pub mod personal;

use anyhow::Result;
use async_trait::async_trait;

use crate::netdisk::NetdiskClient;
use crate::transfer::{
    ShareFileListResult, ShareKind, ShareLink, SharePageInfo, SharedFileInfo, TransferResult,
};

/// 一次分享类 HTTP 请求的描述
///
/// 由 [`NetdiskClient::send_share_request`] 执行，两个 provider 只负责填这个结构，
/// 不各自重复写「带不带 randsk Cookie / 代理成功失败计数 / 读响应文本」那套样板。
pub struct ShareHttpRequest<'a> {
    /// 完整请求 URL（含 query）
    pub url: String,
    /// Referer 头，百度分享接口对它敏感
    pub referer: String,
    /// `None` 发 GET；`Some` 发 POST 表单
    pub form: Option<Vec<(&'a str, String)>>,
    /// 要覆盖进 Cookie 的凭据（个人版 `randsk`）
    ///
    /// 企业版把凭据放在表单的 `spwd` 字段里，这里传 `None`。
    pub cookie_randsk: Option<&'a str>,
    /// 发送失败时附加的错误上下文
    pub error_context: &'static str,
}

/// 一套分享体系的接入策略
///
/// 实现者只需要关心「请求怎么拼、响应怎么拆」，
/// HTTP 发送、代理回退、Cookie 覆盖由 [`NetdiskClient::send_share_request`] 统一处理。
#[async_trait]
pub trait ShareProvider: Send + Sync {
    /// 本策略对应的体系类型
    fn kind(&self) -> ShareKind;

    /// 尝试把 URL 解析成本体系的分享链接
    ///
    /// 不属于本体系时返回 `Ok(None)`，由 [`parse_share_link`] 继续尝试下一个策略。
    fn parse_link(&self, url: &str) -> Result<Option<ShareLink>>;

    /// 访问分享页，取回 `shareid` / `share_uk` / `bdstoken`
    ///
    /// `first` 只影响 Referer 的构造（模拟从网盘首页还是从提取码页跳转）。
    async fn access_page(
        &self,
        client: &NetdiskClient,
        link: &ShareLink,
        first: bool,
    ) -> Result<SharePageInfo>;

    /// 校验提取码，返回后续请求要携带的凭据
    ///
    /// 个人版返回 `randsk`，企业版返回 `spwd`——两者在上层统一走 `randsk` 通道。
    async fn verify_password(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        password: &str,
    ) -> Result<String>;

    /// 列出分享根目录
    async fn list_root(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        page: u32,
        num: u32,
        token: Option<&str>,
    ) -> Result<ShareFileListResult>;

    /// 列出分享中指定子目录
    async fn list_dir(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        dir: &str,
        page: u32,
        num: u32,
        token: Option<&str>,
    ) -> Result<Vec<SharedFileInfo>>;

    /// 转存选中文件到自己网盘
    async fn transfer(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        fs_ids: &[u64],
        target_path: &str,
        internal_task_id: Option<&str>,
        token: Option<&str>,
    ) -> Result<TransferResult>;
}

/// 按体系类型取对应策略
pub fn provider_for(kind: ShareKind) -> &'static dyn ShareProvider {
    match kind {
        ShareKind::Personal => &personal::PersonalShare,
        ShareKind::Apaas => &apaas::ApaasShare,
    }
}

/// 解析分享链接，自动判定体系
///
/// **顺序敏感**：必须先试企业版。企业版链接形如
/// `pan.baidu.com/apaas/share?surl=xxx`，个人版解析器的 `[?&]surl=` 分支
/// 同样能匹配上它并错误地补出 `1xxx` 短链（issue #139 的直接原因）。
pub fn parse_share_link(url: &str) -> Result<ShareLink> {
    let url = url.trim();

    if !url.contains("pan.baidu.com") && !url.contains("baidu.com/s/") {
        anyhow::bail!("无效的分享链接：不是百度网盘链接");
    }

    for provider in [
        provider_for(ShareKind::Apaas),
        provider_for(ShareKind::Personal),
    ] {
        if let Some(link) = provider.parse_link(url)? {
            tracing::info!(
                "解析分享链接成功: kind={:?}, short_key={}, has_password={}",
                link.kind,
                link.short_key,
                link.password.is_some()
            );
            return Ok(link);
        }
    }

    anyhow::bail!("无法从链接中提取分享 ID")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apaas_link_keeps_surl_without_prefix() {
        let link =
            parse_share_link("https://pan.baidu.com/apaas/share?surl=KqfOZ0gupO45mF7TyXzlRg&pwd=1HYy")
                .unwrap();
        assert_eq!(link.kind, ShareKind::Apaas);
        // 关键：不能补 "1" 前缀
        assert_eq!(link.short_key, "KqfOZ0gupO45mF7TyXzlRg");
        assert_eq!(link.password.as_deref(), Some("1HYy"));
    }

    #[test]
    fn personal_short_link_is_unchanged() {
        let link = parse_share_link("https://pan.baidu.com/s/1abcDEFg").unwrap();
        assert_eq!(link.kind, ShareKind::Personal);
        assert_eq!(link.short_key, "1abcDEFg");
        assert!(link.password.is_none());
    }

    #[test]
    fn personal_surl_form_still_gets_prefix() {
        let link =
            parse_share_link("https://pan.baidu.com/share/init?surl=abcDEFg&pwd=xy12").unwrap();
        assert_eq!(link.kind, ShareKind::Personal);
        assert_eq!(link.short_key, "1abcDEFg");
        assert_eq!(link.password.as_deref(), Some("xy12"));
    }

    #[test]
    fn rejects_non_baidu_link() {
        assert!(parse_share_link("https://example.com/s/1abc").is_err());
    }
}
