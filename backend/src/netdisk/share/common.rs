//! 两套分享体系的公共件
//!
//! 个人版和企业版的差异只在「请求怎么拼」和「响应外层信封」，
//! 内层的文件项结构、异步任务轮询节奏、转存结果提取逻辑完全一致。
//! 这些共用部分集中在这里，两个 provider 直接复用，避免各写一遍。

use serde_json::Value;

use crate::transfer::SharedFileInfo;

/// 从 JSON 值里取字符串，兼容字符串和数字两种表示
///
/// 百度的接口对同一个字段时而返回 `123`、时而返回 `"123"`，
/// 个人版的 `fs_id`/`size` 和企业版的 `fsid` 都有这个毛病。
pub fn json_str_or_num(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 取 u64，兼容字符串和数字
pub fn json_u64(value: &Value) -> u64 {
    if let Some(n) = value.as_u64() {
        n
    } else if let Some(s) = value.as_str() {
        s.parse::<u64>().unwrap_or(0)
    } else {
        0
    }
}

/// 取 bool，兼容 `1`/`"1"`/`true`
pub fn json_flag(value: &Value) -> bool {
    if let Some(n) = value.as_i64() {
        n == 1
    } else if let Some(s) = value.as_str() {
        s == "1"
    } else {
        value.as_bool().unwrap_or(false)
    }
}

/// 解析文件列表项
///
/// 个人版的键是 `fs_id`，企业版是 `fsid`（且恒为字符串），
/// 其余 `isdir` / `path` / `size` / `server_filename` 两边一致，
/// 所以这里两个键都试一遍，一个函数吃下两套响应。
pub fn parse_shared_files(list: &[Value]) -> Vec<SharedFileInfo> {
    list.iter()
        .map(|item| {
            // 企业版用 fsid，个人版用 fs_id
            let fs_id_value = if item.get("fs_id").is_some() {
                &item["fs_id"]
            } else {
                &item["fsid"]
            };

            SharedFileInfo {
                fs_id: json_u64(fs_id_value),
                is_dir: json_flag(&item["isdir"]),
                path: item["path"].as_str().unwrap_or_default().to_string(),
                size: json_u64(&item["size"]),
                name: item["server_filename"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            }
        })
        .collect()
}

/// 从转存/任务查询响应的条目列表里提取结果三元组
///
/// 返回 `(转存后路径, 转存前路径, 转存后 fs_id)`，三者按下标对应。
/// 个人版的 `extra.list`、`taskquery` 的 `list`、企业版的 `list`
/// 都是同一套 `{from, to, to_fs_id}` 结构。
pub fn collect_transfer_entries(list: &[Value]) -> (Vec<String>, Vec<String>, Vec<u64>) {
    let mut to_paths = Vec::new();
    let mut from_paths = Vec::new();
    let mut fs_ids = Vec::new();

    for item in list {
        if let Some(path) = item["to"].as_str() {
            to_paths.push(path.to_string());
        }
        if let Some(from) = item["from"].as_str() {
            from_paths.push(from.to_string());
        }
        if let Some(fsid) = item["to_fs_id"].as_u64() {
            fs_ids.push(fsid);
        }
    }

    (to_paths, from_paths, fs_ids)
}

/// 异步转存任务的阶梯式轮询延迟（毫秒，含随机抖动）
///
/// | 尝试次数 | 基础间隔 | 抖动 | 实际区间 |
/// |---|---|---|---|
/// | 1 | 0 | 无 | 立即 |
/// | 2-5 | 1s | ±200ms | 0.8-1.2s |
/// | 6-10 | 2s | ±400ms | 1.6-2.4s |
/// | 11+ | 5s | ±1000ms | 4.0-6.0s |
///
/// 抖动是为了避免多任务同时轮询打出整齐的请求波形触发风控。
pub fn poll_delay_ms(attempt: u32) -> u64 {
    use rand::Rng;

    if attempt <= 1 {
        return 0;
    }

    let (base_ms, jitter_ms) = match attempt {
        2..=5 => (1000i64, 200i64),
        6..=10 => (2000, 400),
        _ => (5000, 1000),
    };

    let mut rng = rand::thread_rng();
    let jitter = rng.gen_range(-jitter_ms..=jitter_ms);
    (base_ms + jitter).max(0) as u64
}

/// 判断转存响应里的 `task_id` 是否代表一个真正的异步任务
///
/// 百度用 `task_id` 非 0 表示转存转入异步执行，但类型时而是数字时而是字符串。
pub fn is_async_task_id(value: &Value) -> bool {
    if let Some(s) = value.as_str() {
        !s.is_empty() && s != "0"
    } else if value.is_u64() || value.is_i64() {
        value.as_u64().unwrap_or(0) != 0
    } else {
        false
    }
}

/// 把 `task_id` 取成字符串（数字/字符串通吃）
pub fn task_id_string(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        s.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_personal_and_apaas_file_items() {
        // 个人版：fs_id 数字、isdir 数字
        let personal = json!([{
            "fs_id": 123456789u64,
            "isdir": 1,
            "path": "/a/b",
            "size": 0,
            "server_filename": "b"
        }]);
        let files = parse_shared_files(personal.as_array().unwrap());
        assert_eq!(files[0].fs_id, 123456789);
        assert!(files[0].is_dir);
        assert_eq!(files[0].name, "b");

        // 企业版：fsid 字符串、size 数字
        let apaas = json!([{
            "fsid": "250200452845377",
            "isdir": 0,
            "path": "/x/y.mp4",
            "size": 382557048u64,
            "server_filename": "y.mp4"
        }]);
        let files = parse_shared_files(apaas.as_array().unwrap());
        assert_eq!(files[0].fs_id, 250200452845377);
        assert!(!files[0].is_dir);
        assert_eq!(files[0].size, 382557048);
    }

    #[test]
    fn collects_transfer_entries_in_order() {
        let list = json!([
            {"from": "/s/1.mp4", "to": "/d/1.mp4", "to_fs_id": 11u64},
            {"from": "/s/2.mp4", "to": "/d/2.mp4", "to_fs_id": 22u64}
        ]);
        let (to, from, ids) = collect_transfer_entries(list.as_array().unwrap());
        assert_eq!(to, vec!["/d/1.mp4", "/d/2.mp4"]);
        assert_eq!(from, vec!["/s/1.mp4", "/s/2.mp4"]);
        assert_eq!(ids, vec![11, 22]);
    }

    #[test]
    fn detects_async_task_id_across_types() {
        assert!(is_async_task_id(&json!(12345u64)));
        assert!(is_async_task_id(&json!("12345")));
        assert!(!is_async_task_id(&json!(0)));
        assert!(!is_async_task_id(&json!("0")));
        assert!(!is_async_task_id(&json!("")));
        assert!(!is_async_task_id(&json!(null)));
    }

    #[test]
    fn poll_delay_follows_ladder() {
        assert_eq!(poll_delay_ms(1), 0);
        assert!((800..=1200).contains(&poll_delay_ms(3)));
        assert!((1600..=2400).contains(&poll_delay_ms(8)));
        assert!((4000..=6000).contains(&poll_delay_ms(20)));
    }
}
