//! 个人版分享策略
//!
//! 对应 `https://pan.baidu.com/s/1xxxx`（以及等价的 `/share/init?surl=xxxx`）。
//!
//! 这套接口在本项目里已经稳定运行很久，踩过的坑（errno=-32 空间不足要早停、
//! `target_file_nums_limit` 缺失时不能误判超限、randsk 走临时客户端等）都沉淀在
//! [`NetdiskClient`] 上的 `*_personal` 系列方法里。**这里只做参数适配和委托，
//! 不复制任何实现**，避免同一套逻辑出现两份而后续只改了其中一份。

use anyhow::Result;
use async_trait::async_trait;
use tracing::info;

use super::ShareProvider;
use crate::netdisk::NetdiskClient;
use crate::transfer::{
    ShareFileListResult, ShareKind, ShareLink, SharePageInfo, SharedFileInfo, TransferResult,
};

/// 个人版分享策略
pub struct PersonalShare;

/// 个人版分享页的 Referer
fn share_referer(short_key: &str) -> String {
    format!("https://pan.baidu.com/s/{}", short_key)
}

#[async_trait]
impl ShareProvider for PersonalShare {
    fn kind(&self) -> ShareKind {
        ShareKind::Personal
    }

    fn parse_link(&self, url: &str) -> Result<Option<ShareLink>> {
        use regex::Regex;

        // 防御性拦截：企业版链接同样带 `surl=`，被本策略吃掉就会补出错误的
        // `1<surl>` 短链（issue #139）。正常流程里 `share::parse_share_link`
        // 已经先试过企业版，这里再挡一道，保证策略单独调用时也是安全的。
        if url.contains("/apaas/share") || url.contains("/netdisk/share") {
            return Ok(None);
        }

        let mut short_key: Option<String> = None;

        // /s/{key} 形式
        let re_s = Regex::new(r"/s/([a-zA-Z0-9_-]+)")?;
        if let Some(caps) = re_s.captures(url) {
            if let Some(key) = caps.get(1) {
                short_key = Some(key.as_str().to_string());
            }
        }

        // /share/init?surl={key} 形式：surl 缺 "1" 前缀，要补上
        if short_key.is_none() {
            let re_surl = Regex::new(r"[?&]surl=([a-zA-Z0-9_-]+)")?;
            if let Some(caps) = re_surl.captures(url) {
                if let Some(key) = caps.get(1) {
                    short_key = Some(format!("1{}", key.as_str()));
                }
            }
        }

        let Some(short_key) = short_key else {
            return Ok(None);
        };

        let re_pwd = Regex::new(r"[?&]pwd=([a-zA-Z0-9]{4})")?;
        let password = re_pwd
            .captures(url)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());

        Ok(Some(ShareLink {
            short_key,
            raw_url: url.to_string(),
            password,
            kind: ShareKind::Personal,
        }))
    }

    async fn access_page(
        &self,
        client: &NetdiskClient,
        link: &ShareLink,
        first: bool,
    ) -> Result<SharePageInfo> {
        client
            .access_share_page_personal(&link.short_key, &link.password, first)
            .await
    }

    async fn verify_password(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        password: &str,
    ) -> Result<String> {
        let randsk = client
            .verify_share_password_personal(
                &info.shareid,
                &info.share_uk,
                &info.bdstoken,
                password,
                &share_referer(&info.short_key),
            )
            .await?;
        info!("个人版提取码验证成功");
        Ok(randsk)
    }

    async fn list_root(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        page: u32,
        num: u32,
        token: Option<&str>,
    ) -> Result<ShareFileListResult> {
        client
            .list_share_files_personal(&info.short_key, &info.bdstoken, page, num, token)
            .await
    }

    async fn list_dir(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        dir: &str,
        page: u32,
        num: u32,
        token: Option<&str>,
    ) -> Result<Vec<SharedFileInfo>> {
        client
            .list_share_files_in_dir_personal(
                &info.short_key,
                &info.shareid,
                &info.uk,
                &info.bdstoken,
                dir,
                page,
                num,
                token,
            )
            .await
    }

    async fn transfer(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        fs_ids: &[u64],
        target_path: &str,
        internal_task_id: Option<&str>,
        token: Option<&str>,
    ) -> Result<TransferResult> {
        client
            .transfer_share_files_personal(
                &info.shareid,
                &info.share_uk,
                &info.bdstoken,
                fs_ids,
                target_path,
                &share_referer(&info.short_key),
                internal_task_id,
                token,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_short_link_and_password() {
        let link = PersonalShare
            .parse_link("https://pan.baidu.com/s/1abcDEFg?pwd=xy12")
            .unwrap()
            .unwrap();
        assert_eq!(link.short_key, "1abcDEFg");
        assert_eq!(link.password.as_deref(), Some("xy12"));
        assert_eq!(link.kind, ShareKind::Personal);
    }

    #[test]
    fn adds_prefix_for_surl_form() {
        let link = PersonalShare
            .parse_link("https://pan.baidu.com/share/init?surl=abcDEFg")
            .unwrap()
            .unwrap();
        assert_eq!(link.short_key, "1abcDEFg");
    }

    #[test]
    fn refuses_apaas_links() {
        assert!(PersonalShare
            .parse_link("https://pan.baidu.com/apaas/share?surl=abc&pwd=1234")
            .unwrap()
            .is_none());
    }
}
