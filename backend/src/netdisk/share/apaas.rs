//! 企业版（apaas）分享策略
//!
//! 对应百度网盘企业版的分享链接 `https://pan.baidu.com/apaas/share?surl=xxx&pwd=yyyy`。
//!
//! 接口取自企业版分享前端 bundle（`share-platform-static/public/b2c-share`），
//! 与个人版没有任何共用端点：
//!
//! ```text
//! GET  /apaas/share?surl=<surl>                     分享页，window.yunData 是干净 JSON
//! POST /apaas/api/share/getspwd?short_url=<surl>    验证提取码 → spwd
//! POST /apaas/1.0/share/list?short_url=<surl>       列目录
//! POST /apaas/1.0/share/transfer                    转存（data=<JSON>）
//! POST /apaas/1.0/share/taskquery                   异步转存任务轮询
//! ```
//!
//! 与个人版的三个易错差异，改动时注意：
//! - `surl` **不加 `1` 前缀**，直接作为 `short_url` 用；
//! - 列根目录时 `dir` 必须为空或不传，传 `/` 会返回 `errno=13042`；
//! - 文件项的键是 `fsid` 且恒为字符串（个人版是 `fs_id`，可能是数字）。

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info, warn};

use super::common;
use super::{ShareHttpRequest, ShareProvider};
use crate::auth::constants::BAIDU_APP_ID;
use crate::netdisk::NetdiskClient;
use crate::transfer::{
    ShareFileListResult, ShareKind, ShareLink, SharePageInfo, SharedFileInfo, TransferResult,
};

/// 企业版分享策略
pub struct ApaasShare;

/// 「部分条目失败」——真正的原因在响应的 `info[]` 内层 errno 里
///
/// 与个人版的 `errno=12` 同义，两套体系在这一点上是一致的。
const PARTIAL_FAILURE_ERRNO: i64 = 12;

/// 企业版接口的公共 query 串（与前端 axios 默认参数一致）
fn common_query() -> String {
    format!(
        "clienttype=0&web=1&channel=chunlei&app_id={}",
        BAIDU_APP_ID
    )
}

/// 企业版分享页的 Referer
fn share_referer(short_url: &str) -> String {
    format!("https://pan.baidu.com/apaas/share?surl={}", short_url)
}

/// 从 HTML 里抠出 `marker` 之后第一个完整的 JSON 对象
///
/// 不能用「非贪婪匹配到第一个 `}`」那种正则：`yunData` 里有多层嵌套。
/// 这里按花括号深度扫描，并跳过字符串字面量中的括号与转义字符。
fn extract_json_object_after(haystack: &str, marker: &str) -> Option<String> {
    let start = haystack.find(marker)? + marker.len();
    let rest = &haystack[start..];
    let open = rest.find('{')?;
    let bytes = rest.as_bytes();

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for i in open..bytes.len() {
        let c = bytes[i];

        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }

        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(rest[open..=i].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

/// 把企业版转存错误码翻译成上层分批逻辑**听得懂**的文案
///
/// `transfer::manager` 的自适应分批全部按错误串里的关键字分派：
///
/// | 判定函数 | 关键字 | 行为 |
/// |---|---|---|
/// | `is_file_limit_exceeded` | `转存文件数` + `超过上限` | 下钻子树、拆批重提 |
/// | `is_quota_exceeded` | `空间不足` | 立即停手，不再拆分 |
/// | `is_transient_transfer_error` | `超时` / `稍后再试` … | 退避重试同一批 |
///
/// 企业版如果只吐一个 errno 数字，这三条全部匹配不上：超限不会拆批而是整批
/// 失败，抖动也不会重试。所以这里必须把企业版错误码映射回同一套词汇。
///
/// 码表取自企业版分享前端 bundle（模块 `7458`）。好在两套体系的转存错误码
/// 高度重合（`-30`/`-31`/`-32`/`-33` 含义完全一致），企业版只是多了几个
/// 五位数别名。
///
/// 未知错误码保持原样透出。这类串会被 `is_quota_exceeded` 的 `errno=-12`
/// 之类兜底关键字碰巧命中的概率极低，且即便命中，「早停」也是比「无限二分」
/// 更安全的失败方向。
pub(crate) fn describe_transfer_errno(errno: i64, show_msg: &str) -> String {
    let detail = if show_msg.is_empty() {
        String::new()
    } else {
        format!("（{}）", show_msg)
    };

    match errno {
        // 空间不足：拆多小都没用，必须早停
        -10 | -32 | 31112 => format!("网盘空间不足，无法转存{}", detail),
        // 文件数超限：交给上层下钻拆批
        -33 | 120 | 130 | 31075 | 31174 | 31175 => {
            format!("转存文件数超过上限{}", detail)
        }
        // 同名冲突。措辞必须含「已存在同名」：share_sync 的 `is_already_exists`
        // 按这个子串把它识别成「网盘里本来就有」而不是真失败（否则重复同步会一直报错）。
        -8 | -30 | 31061 => format!("目标位置已存在同名文件{}", detail),
        // 临时性：超时 / 已有任务在跑，退避重试即可
        4 | -31 | 31069 | 111 | 31171 => {
            format!("转存超时，请稍后再试{}", detail)
        }
        -6 => format!("登录已失效，请重新登录{}", detail),
        20025 => format!("分享页面已失效，请刷新后重试{}", detail),
        90003 => format!("部分文件有版权限制，不支持转存{}", detail),
        other => format!("企业版转存失败: errno={}{}", other, detail),
    }
}

/// 构造企业版转存请求的表单字段
///
/// **字段必须平铺，不能包在 `data` 里**。企业版前端用的是自定义序列化器
/// （bundle 模块 `e487`），不是 JSON：
///
/// ```js
/// if (Array.isArray(r))       push(key + "=" + encodeURIComponent(JSON.stringify(r)));
/// else if (isPlainObject(r))  recurse(r);   // ← 对象递归展平，外层键名被丢弃
/// else                        push(key + "=" + encodeURIComponent(r));
/// ```
///
/// 所以前端写的 `post(url, {data: payload})` 实际发出的是 payload 的字段本身，
/// `data` 这一层根本不出现。照字面发 `data=<整个JSON>` 会得到
/// `errno=2 param error`（实测）。数组则要单独 JSON 化成字符串。
fn transfer_form<'a>(
    fsid_list: &[String],
    target_path: &str,
    spwd: &str,
    short_url: &str,
) -> Vec<(&'a str, String)> {
    vec![
        // 数组按前端规则序列化成 JSON 字符串，如 ["250200452845377"]
        (
            "fsid_list",
            serde_json::to_string(fsid_list).unwrap_or_else(|_| "[]".to_string()),
        ),
        ("to_path", target_path.to_string()),
        ("spwd", spwd.to_string()),
        ("short_url", short_url.to_string()),
        ("async", "1".to_string()),
        ("ondup", "newcopy".to_string()),
        ("product", String::new()),
    ]
}

/// 规范化子目录路径
///
/// 个人版列子目录用的是 `/sharelink{uk}-{shareid}/子路径` 这种带前缀的形式，
/// 企业版只认列表响应里原样的 `path`，带前缀会被判成
/// `errno=13042 prohibit list dir that are not in the sharing link`（实测）。
///
/// 前端已按 `kind` 分别构造，这里再兜一道：调用方（老前端缓存、其它入口）
/// 传进个人版形状时自动剥掉前缀，而不是抛一个看不懂的错。
fn normalize_dir(dir: &str) -> &str {
    let Some(rest) = dir.strip_prefix("/sharelink") else {
        return dir;
    };
    // 前缀形如 sharelink<uk>-<shareid>，后面要么结束、要么接 '/'
    match rest.find('/') {
        Some(i) if rest[..i].chars().all(|c| c.is_ascii_digit() || c == '-') => &rest[i..],
        // 只有前缀没有子路径 → 等价于分享根
        None if rest.chars().all(|c| c.is_ascii_digit() || c == '-') => "",
        _ => dir,
    }
}

/// 按目标路径回查转存后的 fs_id
///
/// 企业版任务查询返回的条目只有 `from` / `to` / `size`，没有个人版那个
/// `to_fs_id`。而上层的自动下载是拿 fs_id 取 dlink 的，缺了就只能得到
/// `fs_id=0`、下载必失败。这里按父目录批量列一次自己网盘，把 fs_id 补回去，
/// 使 `TransferResult` 对上层保持与个人版一致的契约（下游代码无需区分体系）。
///
/// 转存批次本来就按目标目录分组，通常只产生 1 次额外请求。
/// 回查失败不致命：填 0 并告警，让上层照常拿到路径信息。
async fn resolve_fs_ids_by_path(client: &NetdiskClient, paths: &[String]) -> Vec<u64> {
    use std::collections::{HashMap, HashSet};

    if paths.is_empty() {
        return Vec::new();
    }

    // 按父目录去重，避免同一目录列多次
    let parents: HashSet<&str> = paths
        .iter()
        .map(|p| match p.rfind('/') {
            Some(0) | None => "/",
            Some(i) => &p[..i],
        })
        .collect();

    const PAGE_SIZE: u32 = 1000;
    let mut by_path: HashMap<String, u64> = HashMap::new();

    for dir in parents {
        let mut page = 1u32;
        loop {
            match client.get_file_list(dir, page, PAGE_SIZE).await {
                Ok(resp) => {
                    let batch_len = resp.list.len();
                    for item in resp.list {
                        by_path.insert(item.path, item.fs_id);
                    }
                    if (batch_len as u32) < PAGE_SIZE {
                        break;
                    }
                    page += 1;
                }
                Err(e) => {
                    warn!("回查企业版转存 fs_id 失败: dir={}, error={}", dir, e);
                    break;
                }
            }
        }
    }

    paths
        .iter()
        .map(|p| {
            let fs_id = by_path.get(p).copied().unwrap_or(0);
            if fs_id == 0 {
                warn!("未能回查到转存后的 fs_id: path={}", p);
            }
            fs_id
        })
        .collect()
}

/// 解析企业版接口的错误信封
///
/// 企业版用五位数错误码（如 `13042`），与个人版的负数码完全是两套，
/// 不能套用个人版的映射表。好在它的 `show_msg` 基本可读，直接透出即可。
///
/// **`action` 刻意不与 `ShareHttpRequest::error_context` 同名**：后者描述的是
/// 网络发送/读取失败（可重试），会被 `share_sync::error` 的 TRANSIENT_KEYWORDS
/// 命中；而这里是服务端明确返回的 errno（如 13042 目录不在分享内），属于确定性
/// 失败，重试无用。个人版也是这么区分的（网络层「获取分享文件列表失败」
/// vs errno 层「获取文件列表失败」）。改文案时别把两者拉平。
fn check_errno(json: &Value, action: &str) -> Result<()> {
    let errno = json["errno"].as_i64().unwrap_or(-1);
    if errno == 0 {
        return Ok(());
    }

    let show_msg = json["show_msg"].as_str().unwrap_or("");
    let hint = match errno {
        13042 => "，请求的目录不在分享范围内",
        -6 => "，登录状态失效，请重新登录",
        _ => "",
    };

    anyhow::bail!(
        "{}失败: errno={}{}{}",
        action,
        errno,
        hint,
        if show_msg.is_empty() {
            String::new()
        } else {
            format!("（{}）", show_msg)
        }
    )
}

#[async_trait]
impl ShareProvider for ApaasShare {
    fn kind(&self) -> ShareKind {
        ShareKind::Apaas
    }

    fn parse_link(&self, url: &str) -> Result<Option<ShareLink>> {
        use regex::Regex;

        // 只认企业版路由：/apaas/share 或 /netdisk/share（同一套前端的两个入口）
        if !url.contains("/apaas/share") && !url.contains("/netdisk/share") {
            return Ok(None);
        }

        let re_surl = Regex::new(r"[?&]surl=([a-zA-Z0-9_-]+)")?;
        let Some(short_key) = re_surl
            .captures(url)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
        else {
            return Ok(None);
        };

        // 企业版提取码同样是 4 位
        let re_pwd = Regex::new(r"[?&]pwd=([a-zA-Z0-9]{4})")?;
        let password = re_pwd
            .captures(url)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());

        Ok(Some(ShareLink {
            short_key,
            raw_url: url.to_string(),
            password,
            kind: ShareKind::Apaas,
        }))
    }

    async fn access_page(
        &self,
        client: &NetdiskClient,
        link: &ShareLink,
        _first: bool,
    ) -> Result<SharePageInfo> {
        let url = format!(
            "https://pan.baidu.com/apaas/share?surl={}&{}",
            link.short_key,
            common_query()
        );
        info!("访问企业版分享页面: surl={}", link.short_key);

        let body = client
            .send_share_request(ShareHttpRequest {
                url,
                referer: "https://pan.baidu.com/disk/home".to_string(),
                form: None,
                cookie_randsk: None,
                // 「企业版」作后缀而非中缀：`share_sync::error` 的 TRANSIENT_KEYWORDS
                // 按子串匹配这些上下文来判定「网络抖动 vs 链接确定性失效」，
                // 写成「访问企业版分享页面失败」会匹配不上，一次抖动就把订阅自动暂停。
                error_context: "访问分享页面失败（企业版）",
            })
            .await?;

        let raw = extract_json_object_after(&body, "window.yunData")
            .context("企业版分享页面未找到 yunData，链接可能已失效或不是企业版分享")?;
        let data: Value =
            serde_json::from_str(&raw).context("解析企业版分享页面 yunData 失败")?;

        let share_info = &data["shareInfo"];
        let errno = share_info["errno"].as_i64().unwrap_or(-1);
        if errno != 0 {
            let show_msg = share_info["show_msg"].as_str().unwrap_or("");
            anyhow::bail!(
                "企业版分享不可用: errno={}{}",
                errno,
                if show_msg.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", show_msg)
                }
            );
        }

        let link_info = &share_info["link_info"];
        let shareid = common::json_str_or_num(&link_info["share_id"]).unwrap_or_default();
        let share_uk = common::json_str_or_num(&link_info["share_uk"]).unwrap_or_default();

        if shareid.is_empty() {
            anyhow::bail!("无法从企业版分享页面提取 share_id，请确认链接有效");
        }

        // 服务端回传的 short_url 优先（可能与 URL 里的大小写/形式有出入）
        let short_key = link_info["short_url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(&link.short_key)
            .to_string();

        info!(
            "提取企业版分享信息成功: share_id={}, share_uk={}, file_cnt={}",
            shareid,
            share_uk,
            link_info["file_cnt"].as_u64().unwrap_or(0)
        );

        Ok(SharePageInfo {
            shareid,
            uk: share_uk.clone(),
            share_uk,
            // 企业版接口不需要 bdstoken
            bdstoken: String::new(),
            kind: ShareKind::Apaas,
            short_key,
        })
    }

    async fn verify_password(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        password: &str,
    ) -> Result<String> {
        info!("验证企业版提取码: short_url={}", info.short_key);

        let url = format!(
            "https://pan.baidu.com/apaas/api/share/getspwd?short_url={}&product=&{}",
            info.short_key,
            common_query()
        );

        let body = client
            .send_share_request(ShareHttpRequest {
                url,
                referer: share_referer(&info.short_key),
                form: Some(vec![("pwd", password.to_string())]),
                cookie_randsk: None,
                error_context: "验证提取码请求失败（企业版）",
            })
            .await?;

        debug!("企业版验证提取码响应: {}", body);
        let json: Value =
            serde_json::from_str(&body).context("解析企业版验证提取码响应失败")?;

        // 外层信封 + 内层 data 都有 errno，内层才是提取码校验结果
        check_errno(&json, "企业版验证提取码")?;

        let data = &json["data"];
        let inner_errno = data["errno"].as_i64().unwrap_or(-1);
        let spwd = data["spwd"].as_str().unwrap_or_default();

        if inner_errno != 0 || spwd.is_empty() {
            let show_msg = data["show_msg"].as_str().unwrap_or("");
            // 上层按「提取码错误」关键字回报给前端，保持与个人版一致的文案
            anyhow::bail!(
                "提取码错误: errno={}{}",
                inner_errno,
                if show_msg.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", show_msg)
                }
            );
        }

        info!("企业版提取码验证成功，已获取 spwd");
        Ok(spwd.to_string())
    }

    async fn list_root(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        page: u32,
        num: u32,
        token: Option<&str>,
    ) -> Result<ShareFileListResult> {
        // 根目录：dir 传空。传 "/" 会被服务端判成「不在分享范围内」（errno=13042）
        let json = self
            .request_list(client, info, "", page, num, token)
            .await?;

        let data = &json["data"];
        let files = data["list"]
            .as_array()
            .map(|l| common::parse_shared_files(l))
            .unwrap_or_default();

        let uk = common::json_str_or_num(&data["uk"]).unwrap_or_default();

        info!(
            "企业版根目录: {} 个文件/文件夹, uk={}",
            files.len(),
            uk
        );

        Ok(ShareFileListResult {
            files,
            uk,
            shareid: info.shareid.clone(),
            // 企业版列表响应没有 title 字段，share_root 交给上层的启发式推导
            share_root_path: None,
        })
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
        let json = self.request_list(client, info, dir, page, num, token).await?;

        let files = json["data"]["list"]
            .as_array()
            .map(|l| common::parse_shared_files(l))
            .unwrap_or_default();

        info!("企业版子目录: {} 个文件, dir={}", files.len(), dir);
        Ok(files)
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
        // fsid_list 用字符串数组：企业版列表返回的 fsid 本身就是字符串，
        // 大 fsid 走数字在部分 JSON 实现里会丢精度。
        let fsid_list: Vec<String> = fs_ids.iter().map(|id| id.to_string()).collect();

        let url = format!(
            "https://pan.baidu.com/apaas/1.0/share/transfer?{}",
            common_query()
        );

        let body = client
            .send_share_request(ShareHttpRequest {
                url,
                referer: share_referer(&info.short_key),
                form: Some(transfer_form(
                    &fsid_list,
                    target_path,
                    token.unwrap_or_default(),
                    &info.short_key,
                )),
                cookie_randsk: None,
                error_context: "转存请求失败（企业版）",
            })
            .await?;

        info!("企业版转存响应: {}", body);
        let json: Value = serde_json::from_str(&body).context("解析企业版转存响应失败")?;

        let errno = json["errno"].as_i64().unwrap_or(-1);
        if errno != 0 {
            let show_msg = json["show_msg"].as_str().unwrap_or("");

            // errno=12 是「部分条目失败」，真正的原因在 info[] 内层码里，
            // 与个人版的 errno=12 + info[].errno 结构一致。取第一个非 0 的内层码。
            let effective_errno = if errno == PARTIAL_FAILURE_ERRNO {
                json["info"]
                    .as_array()
                    .or_else(|| json["data"]["info"].as_array())
                    .and_then(|list| {
                        list.iter()
                            .filter_map(|it| it["errno"].as_i64())
                            .find(|&e| e != 0)
                    })
                    .unwrap_or(errno)
            } else {
                errno
            };

            let error = describe_transfer_errno(effective_errno, show_msg);
            warn!(
                "企业版转存失败: errno={}, effective_errno={}, error={}",
                errno, effective_errno, error
            );
            return Ok(TransferResult {
                success: false,
                transferred_paths: vec![],
                from_paths: vec![],
                error: Some(error),
                transferred_fs_ids: vec![],
            });
        }

        // async=1 时服务端返回 task_id，转异步轮询
        let task_id_value = if json["task_id"].is_null() {
            &json["data"]["task_id"]
        } else {
            &json["task_id"]
        };

        if common::is_async_task_id(task_id_value) {
            let task_id = common::task_id_string(task_id_value);
            info!(
                "企业版异步转存任务: baidu_task_id={}, internal_task_id={}, target_path={}",
                task_id,
                internal_task_id.unwrap_or("N/A"),
                target_path
            );
            return self.query_task(client, info, &task_id, internal_task_id).await;
        }

        // 同步返回：结果列表位置与异步分支一致（desc.list 优先）
        let list = json["data"]["desc"]["list"]
            .as_array()
            .or_else(|| json["desc"]["list"].as_array())
            .or_else(|| json["data"]["list"].as_array())
            .or_else(|| json["list"].as_array());
        let (transferred_paths, from_paths, _) =
            list.map(|l| common::collect_transfer_entries(l)).unwrap_or_default();

        // 同样没有 to_fs_id，按路径回查补齐
        let transferred_fs_ids = resolve_fs_ids_by_path(client, &transferred_paths).await;

        Ok(TransferResult {
            success: true,
            transferred_paths,
            from_paths,
            error: None,
            transferred_fs_ids,
        })
    }
}

impl ApaasShare {
    /// 列目录请求（根目录与子目录只差一个 `dir` 参数）
    async fn request_list(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        dir: &str,
        page: u32,
        num: u32,
        token: Option<&str>,
    ) -> Result<Value> {
        let url = format!(
            "https://pan.baidu.com/apaas/1.0/share/list?short_url={}&product=&{}",
            info.short_key,
            common_query()
        );

        let dir = normalize_dir(dir);

        let mut form = vec![
            ("spwd", token.unwrap_or_default().to_string()),
            ("page", page.to_string()),
            ("page_size", num.to_string()),
        ];
        // 根目录不能带 dir（传 "/" 会 errno=13042）
        if !dir.is_empty() && dir != "/" {
            form.push(("dir", dir.to_string()));
        }

        let body = client
            .send_share_request(ShareHttpRequest {
                url,
                referer: share_referer(&info.short_key),
                form: Some(form),
                cookie_randsk: None,
                error_context: "获取分享文件列表失败（企业版）",
            })
            .await?;

        debug!("企业版文件列表响应: {}", body);
        let json: Value =
            serde_json::from_str(&body).context("解析企业版文件列表响应失败")?;
        check_errno(&json, "获取企业版分享文件列表")?;
        Ok(json)
    }

    /// 轮询企业版异步转存任务
    ///
    /// 轮询节奏复用 [`common::poll_delay_ms`]，与个人版一致。
    async fn query_task(
        &self,
        client: &NetdiskClient,
        info: &SharePageInfo,
        task_id: &str,
        internal_task_id: Option<&str>,
    ) -> Result<TransferResult> {
        let url = format!(
            "https://pan.baidu.com/apaas/1.0/share/taskquery?{}",
            common_query()
        );

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let delay = common::poll_delay_ms(attempt);
            if delay > 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
            }

            let body = client
                .send_share_request(ShareHttpRequest {
                    url: url.clone(),
                    referer: share_referer(&info.short_key),
                    // 同样是平铺字段，不能包在 data 里（见 transfer_form 的说明）
                    form: Some(vec![("task_id", task_id.to_string())]),
                    cookie_randsk: None,
                    error_context: "任务查询请求失败（企业版）",
                })
                .await?;

            debug!("企业版任务查询响应 (尝试 {}): {}", attempt, body);
            let json: Value =
                serde_json::from_str(&body).context("解析企业版任务查询响应失败")?;

            check_errno(&json, "企业版任务查询")?;

            // 结果可能在顶层，也可能包在 data 里
            let scope = if json["status"].is_null() {
                &json["data"]
            } else {
                &json
            };

            let task_errno = scope["task_errno"].as_i64().unwrap_or(0);
            let status = scope["status"].as_str().unwrap_or("");

            if task_errno != 0 {
                let show_msg = scope["show_msg"].as_str().unwrap_or("");
                warn!(
                    "企业版异步转存任务失败: baidu_task_id={}, internal_task_id={}, task_errno={}, status={}, show_msg='{}'",
                    task_id,
                    internal_task_id.unwrap_or("N/A"),
                    task_errno,
                    status,
                    show_msg
                );
                anyhow::bail!(
                    "企业版异步转存任务失败: task_errno={}, response={}",
                    task_errno,
                    body
                );
            }

            match status {
                "success" => {
                    // 结果在 `data.desc.list`，不是 `data.list`——`desc` 在 running
                    // 阶段是空数组 `[]`，success 时才变成 `{list:[...], succNum:N}`。
                    // 兼容顺序：desc.list → list（万一将来对齐个人版）。
                    let list = scope["desc"]["list"]
                        .as_array()
                        .or_else(|| scope["list"].as_array());
                    let (transferred_paths, from_paths, _) = list
                        .map(|l| common::collect_transfer_entries(l))
                        .unwrap_or_default();

                    // 企业版的结果条目只有 from/to/size，**没有 to_fs_id**（个人版有）。
                    // 下游自动下载要靠 fs_id 取 dlink，这里按目标路径回查补齐，
                    // 让 TransferResult 对上层保持与个人版一致的契约。
                    let transferred_fs_ids =
                        resolve_fs_ids_by_path(client, &transferred_paths).await;

                    info!(
                        "企业版异步转存完成: task_id={}, 尝试次数={}, {} 个条目, 回查到 {} 个 fs_id",
                        task_id,
                        attempt,
                        transferred_paths.len(),
                        transferred_fs_ids.iter().filter(|id| **id != 0).count()
                    );

                    return Ok(TransferResult {
                        success: true,
                        transferred_paths,
                        from_paths,
                        error: None,
                        transferred_fs_ids,
                    });
                }
                "failed" => {
                    anyhow::bail!("企业版异步转存任务失败: status=failed, response={}", body);
                }
                "running" | "pending" => continue,
                other => {
                    warn!(
                        "企业版异步转存任务状态未知: status='{}', 继续轮询 (尝试 {})",
                        other, attempt
                    );
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_nested_yundata_object() {
        let html = r#"<script>window.yunData = {"a":{"b":[1,2]},"c":"}"};
            window.yunData && eval();</script>"#;
        let raw = extract_json_object_after(html, "window.yunData").unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["a"]["b"][1], 2);
        // 字符串里的 } 不能提前结束扫描
        assert_eq!(v["c"], "}");
    }

    #[test]
    fn extraction_handles_escaped_quotes() {
        let html = r#"window.yunData = {"name":"a\"}b","n":1};"#;
        let raw = extract_json_object_after(html, "window.yunData").unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["n"], 1);
        assert_eq!(v["name"], "a\"}b");
    }

    #[test]
    fn parse_link_only_accepts_apaas_routes() {
        // 个人版链接不该被企业版策略吃掉
        assert!(ApaasShare
            .parse_link("https://pan.baidu.com/s/1abc")
            .unwrap()
            .is_none());
        assert!(ApaasShare
            .parse_link("https://pan.baidu.com/share/init?surl=abc")
            .unwrap()
            .is_none());

        let link = ApaasShare
            .parse_link("https://pan.baidu.com/apaas/share?surl=KqfOZ0gupO45mF7TyXzlRg&pwd=1HYy")
            .unwrap()
            .unwrap();
        assert_eq!(link.short_key, "KqfOZ0gupO45mF7TyXzlRg");
        assert_eq!(link.password.as_deref(), Some("1HYy"));
    }

    /// 转存表单必须平铺。包成 `data=<JSON>` 会被百度判成 `errno=2 param error`
    /// （2026-08-10 实测），因为企业版前端的序列化器会把嵌套对象展平、丢掉外层键。
    #[test]
    fn transfer_form_is_flat_with_json_array() {
        let form = transfer_form(
            &["250200452845377".to_string(), "2804215504560".to_string()],
            "/13/13",
            "SPWD-TOKEN",
            "KqfOZ0gupO45mF7TyXzlRg",
        );
        let map: std::collections::HashMap<_, _> = form.iter().cloned().collect();

        // 绝不能出现 data 这个包装层
        assert!(!map.contains_key("data"), "字段被错误地包进了 data: {:?}", map);

        // 数组单独 JSON 化
        assert_eq!(map["fsid_list"], r#"["250200452845377","2804215504560"]"#);
        assert_eq!(map["to_path"], "/13/13");
        assert_eq!(map["spwd"], "SPWD-TOKEN");
        assert_eq!(map["short_url"], "KqfOZ0gupO45mF7TyXzlRg");
        assert_eq!(map["async"], "1");
        assert_eq!(map["ondup"], "newcopy");
        assert_eq!(map["product"], "");
    }

    /// 任务查询成功时结果在 `data.desc.list`，且条目**没有 to_fs_id**。
    /// 之前读 `data.list` 导致 transferred_paths 为空、进度卡在 0/1（2026-08-10 实测）。
    #[test]
    fn taskquery_success_entries_live_under_desc_list() {
        let body: Value = serde_json::from_str(
            r#"{"data":{"desc":{"list":[{"error_code":0,
                "from":"/build_1125897667178235-9/夫君背叛，局中方醒  首发12集",
                "size":1992141078,
                "to":"/13/13/夫君背叛，局中方醒  首发12集"}],"succNum":1},
                "progress":0,"status":"success","task_id":"507727160105585"},
                "errno":0}"#,
        )
        .unwrap();

        let scope = &body["data"];
        let list = scope["desc"]["list"]
            .as_array()
            .or_else(|| scope["list"].as_array())
            .expect("应能定位到结果列表");
        let (to, from, fs_ids) = common::collect_transfer_entries(list);

        assert_eq!(to, vec!["/13/13/夫君背叛，局中方醒  首发12集"]);
        assert_eq!(from.len(), 1);
        // 企业版不返回 to_fs_id，必须靠 resolve_fs_ids_by_path 回查
        assert!(fs_ids.is_empty(), "企业版本就没有 to_fs_id");
    }

    /// running 阶段 `desc` 是空数组，不能把它误当成结果列表。
    #[test]
    fn taskquery_running_desc_is_empty_array() {
        let body: Value = serde_json::from_str(
            r#"{"data":{"desc":[],"progress":10,"status":"running","task_id":"507727160105585"},"errno":0}"#,
        )
        .unwrap();
        let scope = &body["data"];
        assert_eq!(scope["status"].as_str(), Some("running"));
        assert!(scope["desc"]["list"].as_array().is_none());
    }

    /// 个人版的 `/sharelink{uk}-{shareid}/…` 前缀要剥掉，否则 errno=13042（实测）。
    #[test]
    fn normalize_dir_strips_personal_sharelink_prefix() {
        assert_eq!(
            normalize_dir("/sharelink1644665781-13606518782/夫君背叛，局中方醒  首发12集"),
            "/夫君背叛，局中方醒  首发12集"
        );
        // 只有前缀没有子路径 → 分享根
        assert_eq!(normalize_dir("/sharelink1644665781-13606518782"), "");
        // 企业版原生路径原样通过
        assert_eq!(normalize_dir("/夫君背叛，局中方醒  首发12集"), "/夫君背叛，局中方醒  首发12集");
        assert_eq!(normalize_dir(""), "");
        assert_eq!(normalize_dir("/"), "/");
        // 名字碰巧以 sharelink 开头的真实目录不能被误剥
        assert_eq!(normalize_dir("/sharelinkage/a"), "/sharelinkage/a");
    }

    #[test]
    fn errno_13042_carries_dir_hint() {
        let json: Value = serde_json::from_str(
            r#"{"errno":13042,"show_msg":"prohibit list dir that are not in the sharing link"}"#,
        )
        .unwrap();
        let err = check_errno(&json, "获取企业版分享文件列表").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("13042"));
        assert!(msg.contains("不在分享范围内"));
    }
}
