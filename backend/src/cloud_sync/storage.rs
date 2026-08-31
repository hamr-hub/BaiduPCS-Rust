//! 存储后端抽象 + 三种实现（S3 / OSS / Baidu）
//!
//! S3 与 OSS 都通过手写 SigV4 + reqwest 实现，避免拖入 aws-sdk-s3 / s3 crate
//! （这两个的最新版本要求 rustc ≥1.91，与本仓库固定的 1.87 不兼容）。

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use hmac::{Hmac, Mac};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client as HttpClient, Method, Response, StatusCode as HttpStatus};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncWriteExt;

use crate::cloud_sync::error::{CloudSyncError, Result};
use crate::cloud_sync::types::{
    ConnectionConfig, ListObjectsResult, ObjectInfo, OssConfig, S3Config, StorageKind,
    TestConnectionResult,
};

#[async_trait]
pub trait Storage: Send + Sync {
    fn kind(&self) -> StorageKind;
    async fn test_connection(&self) -> Result<TestConnectionResult>;
    async fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<()>;
    async fn upload_stream(
        &self,
        stream: Box<dyn futures_util::Stream<Item = std::io::Result<Bytes>> + Send + Unpin>,
        remote_path: &str,
    ) -> Result<()>;
    async fn download_file(&self, remote_path: &str, local_path: &Path) -> Result<()>;
    async fn open_read_stream(
        &self,
        remote_path: &str,
    ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>>;
    async fn list_objects(&self, prefix: &str) -> Result<ListObjectsResult>;
    async fn delete_object(&self, remote_path: &str) -> Result<()>;
    async fn head_size(&self, remote_path: &str) -> Result<u64>;
}

type HmacSha256 = Hmac<Sha256>;

struct SigV4Ctx<'a> {
    access_key: &'a str,
    secret_key: &'a str,
    region: &'a str,
    service: &'a str,
}

fn sigv4_sign(
    ctx: &SigV4Ctx,
    method: &Method,
    host: &str,
    canonical_uri: &str,
    canonical_querystring: &str,
    payload_hash: &str,
    amz_date: &str,
    extra_headers: &[(String, String)],
) -> String {
    let mut h = vec![("host".to_string(), host.to_string())];
    h.push(("x-amz-date".to_string(), amz_date.to_string()));
    for (k, v) in extra_headers { h.push((k.to_lowercase(), v.trim().to_string())); }
    h.sort_by(|a, b| a.0.cmp(&b.0));
    let mut canonical_headers = String::new();
    for (k, v) in &h { canonical_headers.push_str(&format!("{}:{}\n", k, v)); }
    let mut signed = vec!["host".to_string(), "x-amz-date".to_string()];
    for (k, _) in extra_headers { signed.push(k.to_lowercase()); }
    signed.sort();
    let signed_headers = signed.join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.as_str(), canonical_uri, canonical_querystring,
        canonical_headers, signed_headers, payload_hash,
    );
    let date_stamp = &amz_date[..8];
    let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, ctx.region, ctx.service);
    let mut hasher = Sha256::new();
    hasher.update(canonical_request.as_bytes());
    let canonical_request_hash = hex::encode(hasher.finalize());
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{}\n{}\n{}", amz_date, credential_scope, canonical_request_hash);
    let hmac_key = |key: &[u8], msg: &[u8]| -> Vec<u8> {
        let mut m = <HmacSha256 as Mac>::new_from_slice(key).expect("hmac key");
        m.update(msg);
        m.finalize().into_bytes().to_vec()
    };
    let k_date = hmac_key(format!("AWS4{}", ctx.secret_key).as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_key(&k_date, ctx.region.as_bytes());
    let k_service = hmac_key(&k_region, ctx.service.as_bytes());
    let k_signing = hmac_key(&k_service, b"aws4_request");
    let signature = hex::encode(hmac_key(&k_signing, string_to_sign.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        ctx.access_key, credential_scope, signed_headers, signature
    )
}

fn amz_date_now() -> String {
    let now: DateTime<Utc> = Utc::now();
    now.format("%Y%m%dT%H%M%SZ").to_string()
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

struct S3HttpClient {
    http: HttpClient,
    cfg: S3Config,
}

impl S3HttpClient {
    fn new(cfg: S3Config) -> Self {
        let http = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(3600))
            .build()
            .expect("reqwest build");
        Self { http, cfg }
    }

    fn host(&self) -> String {
        if self.cfg.path_style.unwrap_or(false) {
            self.cfg.endpoint.as_deref()
                .and_then(|e| url_host(e).map(|s| s.to_string()))
                .unwrap_or_else(|| format!("s3.{}.amazonaws.com", self.cfg.region))
        } else {
            match &self.cfg.endpoint {
                Some(ep) => url_host(ep).unwrap_or(ep).to_string(),
                None => format!("{}.s3.{}.amazonaws.com", self.cfg.bucket, self.cfg.region),
            }
        }
    }

    fn endpoint_url(&self) -> String {
        if let Some(ep) = &self.cfg.endpoint {
            ep.trim_end_matches('/').to_string()
        } else {
            format!("https://s3.{}.amazonaws.com", self.cfg.region)
        }
    }

    async fn head_bucket(&self) -> Result<Response> {
        let url = format!("{}/{}", self.endpoint_url(), self.cfg.bucket);
        let host = self.host();
        let ctx = SigV4Ctx { access_key: &self.cfg.access_key, secret_key: &self.cfg.secret_key, region: &self.cfg.region, service: "s3" };
        let amz = amz_date_now();
        let auth = sigv4_sign(&ctx, &Method::HEAD, &host, "/", "", "UNSIGNED-PAYLOAD", &amz, &[]);
        let resp = self.http.request(Method::HEAD, &url)
            .header("x-amz-date", amz).header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .header(AUTHORIZATION, auth).header("Host", &host).send().await
            .map_err(|e| CloudSyncError::network(format!("HEAD bucket 失败: {}", e)))?;
        Ok(resp)
    }

    async fn list_objects(&self, prefix: &str, max_keys: Option<u32>, continuation: Option<&str>)
        -> Result<(Vec<ObjectInfo>, Vec<String>, bool, Option<String>)>
    {
        let mut qs = vec![("list-type", "2".to_string())];
        if !prefix.is_empty() { qs.push(("prefix", prefix.to_string())); }
        if let Some(mk) = max_keys { qs.push(("max-keys", mk.to_string())); }
        if let Some(c) = continuation { qs.push(("continuation-token", c.to_string())); }
        qs.sort_by(|a, b| a.0.cmp(b.0));
        let canonical_querystring = qs.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect::<Vec<_>>().join("&");
        let url = format!("{}/?{}", self.endpoint_url(), canonical_querystring);
        let host = self.host();
        let ctx = SigV4Ctx { access_key: &self.cfg.access_key, secret_key: &self.cfg.secret_key, region: &self.cfg.region, service: "s3" };
        let amz = amz_date_now();
        let auth = sigv4_sign(&ctx, &Method::GET, &host, "/", &canonical_querystring, "UNSIGNED-PAYLOAD", &amz, &[]);
        let resp = self.http.request(Method::GET, &url)
            .header("x-amz-date", amz).header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .header(AUTHORIZATION, auth).header("Host", &host).send().await
            .map_err(|e| CloudSyncError::network(format!("LIST 失败: {}", e)))?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(|e| CloudSyncError::network(format!("LIST body 失败: {}", e)))?;
        if !status.is_success() { return Err(http_status_err(status, "LIST", &body)); }
        parse_list_response(&body)
    }

    async fn head_object(&self, key: &str) -> Result<Response> {
        let url = format!("{}/{}", self.endpoint_url(), key.trim_start_matches('/'));
        let host = self.host();
        let ctx = SigV4Ctx { access_key: &self.cfg.access_key, secret_key: &self.cfg.secret_key, region: &self.cfg.region, service: "s3" };
        let amz = amz_date_now();
        let auth = sigv4_sign(&ctx, &Method::HEAD, &host, "/", "", "UNSIGNED-PAYLOAD", &amz, &[]);
        let resp = self.http.request(Method::HEAD, &url)
            .header("x-amz-date", amz).header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .header(AUTHORIZATION, auth).header("Host", &host).send().await
            .map_err(|e| CloudSyncError::network(format!("HEAD 失败: {}", e)))?;
        Ok(resp)
    }

    async fn put_object(&self, key: &str, body: Vec<u8>, content_type: Option<&str>) -> Result<()> {
        let url = format!("{}/{}", self.endpoint_url(), key.trim_start_matches('/'));
        let host = self.host();
        let ctx = SigV4Ctx { access_key: &self.cfg.access_key, secret_key: &self.cfg.secret_key, region: &self.cfg.region, service: "s3" };
        let payload_hash = sha256_hex(&body);
        let amz = amz_date_now();
        let mut extra: Vec<(String, String)> = vec![("x-amz-content-sha256".into(), payload_hash.clone())];
        if let Some(ct) = content_type { extra.push(("content-type".into(), ct.to_string())); }
        let auth = sigv4_sign(&ctx, &Method::PUT, &host, "/", "", &payload_hash, &amz, &extra);
        let mut req = self.http.request(Method::PUT, &url)
            .header("x-amz-date", amz).header("x-amz-content-sha256", payload_hash)
            .header(AUTHORIZATION, auth).header("Host", &host).body(body);
        if let Some(ct) = content_type { req = req.header(CONTENT_TYPE, ct); }
        let resp = req.send().await.map_err(|e| CloudSyncError::network(format!("PUT 失败: {}", e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await.unwrap_or_default();
            return Err(http_status_err(status, "PUT", &body));
        }
        Ok(())
    }

    async fn get_object(&self, key: &str) -> Result<Response> {
        let url = format!("{}/{}", self.endpoint_url(), key.trim_start_matches('/'));
        let host = self.host();
        let ctx = SigV4Ctx { access_key: &self.cfg.access_key, secret_key: &self.cfg.secret_key, region: &self.cfg.region, service: "s3" };
        let amz = amz_date_now();
        let auth = sigv4_sign(&ctx, &Method::GET, &host, "/", "", "UNSIGNED-PAYLOAD", &amz, &[]);
        let resp = self.http.request(Method::GET, &url)
            .header("x-amz-date", amz).header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .header(AUTHORIZATION, auth).header("Host", &host).send().await
            .map_err(|e| CloudSyncError::network(format!("GET 失败: {}", e)))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(CloudSyncError::not_found(format!("对象不存在: {}", key)));
        }
        if !status.is_success() {
            let body = resp.bytes().await.unwrap_or_default();
            return Err(http_status_err(status, "GET", &body));
        }
        Ok(resp)
    }

    async fn delete_object(&self, key: &str) -> Result<()> {
        let url = format!("{}/{}", self.endpoint_url(), key.trim_start_matches('/'));
        let host = self.host();
        let ctx = SigV4Ctx { access_key: &self.cfg.access_key, secret_key: &self.cfg.secret_key, region: &self.cfg.region, service: "s3" };
        let amz = amz_date_now();
        let auth = sigv4_sign(&ctx, &Method::DELETE, &host, "/", "", "UNSIGNED-PAYLOAD", &amz, &[]);
        let resp = self.http.request(Method::DELETE, &url)
            .header("x-amz-date", amz).header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .header(AUTHORIZATION, auth).header("Host", &host).send().await
            .map_err(|e| CloudSyncError::network(format!("DELETE 失败: {}", e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await.unwrap_or_default();
            return Err(http_status_err(status, "DELETE", &body));
        }
        Ok(())
    }
}

fn url_host(url: &str) -> Option<&str> {
    let without_scheme = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    host_port.split(':').next()
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn http_status_err(status: HttpStatus, op: &str, body: &[u8]) -> CloudSyncError {
    let msg = format!("{} HTTP {}: {}", op, status, String::from_utf8_lossy(body));
    if status == HttpStatus::NOT_FOUND { CloudSyncError::not_found(msg) }
    else if status == HttpStatus::UNAUTHORIZED || status == HttpStatus::FORBIDDEN { CloudSyncError::auth(msg) }
    else if status == HttpStatus::TOO_MANY_REQUESTS { CloudSyncError::rate_limited(msg) }
    else { CloudSyncError::internal(msg) }
}

fn parse_list_response(body: &[u8]) -> Result<(Vec<ObjectInfo>, Vec<String>, bool, Option<String>)> {
    let s = String::from_utf8_lossy(body).to_string();
    let mut objects = Vec::new();
    let mut prefixes = Vec::new();
    let truncated = s.contains("<IsTruncated>true</IsTruncated>");

    let mut rest = s.as_str();
    while let Some(start) = rest.find("<Contents>") {
        let after = &rest[start + "<Contents>".len()..];
        if let Some(end) = after.find("</Contents>") {
            let block = &after[..end];
            let key = extract_tag(block, "Key").unwrap_or_default();
            let size: u64 = extract_tag(block, "Size").and_then(|v| v.parse().ok()).unwrap_or(0);
            let last_modified = extract_tag(block, "LastModified")
                .and_then(|v| DateTime::parse_from_rfc3339(&v).ok().map(|d| d.with_timezone(&Utc)));
            let etag = extract_tag(block, "ETag");
            objects.push(ObjectInfo { key, size, last_modified, etag });
            rest = &after[end + "</Contents>".len()..];
        } else { break; }
    }

    let mut rest = s.as_str();
    while let Some(start) = rest.find("<CommonPrefixes>") {
        let after = &rest[start + "<CommonPrefixes>".len()..];
        if let Some(end) = after.find("</CommonPrefixes>") {
            let block = &after[..end];
            if let Some(p) = extract_tag(block, "Prefix") { prefixes.push(p); }
            rest = &after[end + "</CommonPrefixes>".len()..];
        } else { break; }
    }

    let next_token = extract_tag(&s, "NextContinuationToken");
    Ok((objects, prefixes, truncated, next_token))
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    Some(xml[s..e].to_string())
}

pub struct S3Storage { client: S3HttpClient }

impl S3Storage {
    pub fn new(cfg: &S3Config) -> Result<Self> {
        Ok(Self { client: S3HttpClient::new(cfg.clone()) })
    }
}

#[async_trait]
impl Storage for S3Storage {
    fn kind(&self) -> StorageKind { StorageKind::S3 }

    async fn test_connection(&self) -> Result<TestConnectionResult> {
        let started = Instant::now();
        let resp = self.client.head_bucket().await?;
        let status = resp.status();
        if !status.is_success() { return Err(http_status_err(status, "HEAD bucket", &[])); }
        let (objs, _, _, _) = self.client.list_objects("", Some(5), None).await?;
        Ok(TestConnectionResult {
            ok: true,
            latency_ms: started.elapsed().as_millis() as u64,
            error: None,
            sample_objects: objs.into_iter().map(|o| o.key).collect(),
        })
    }

    async fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<()> {
        let data = tokio::fs::read(local_path).await?;
        self.client.put_object(remote_path, data, None).await
    }

    async fn upload_stream(&self, mut stream: Box<dyn futures_util::Stream<Item = std::io::Result<Bytes>> + Send + Unpin>, remote_path: &str) -> Result<()> {
        let mut buf = Vec::new();
        while let Some(chunk) = stream.try_next().await? { buf.extend_from_slice(&chunk); }
        self.client.put_object(remote_path, buf, None).await
    }

    async fn download_file(&self, remote_path: &str, local_path: &Path) -> Result<()> {
        let mut resp = self.client.get_object(remote_path).await?;
        if let Some(parent) = local_path.parent() { tokio::fs::create_dir_all(parent).await?; }
        let mut file = tokio::fs::File::create(local_path).await?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| CloudSyncError::network(format!("读取下载流失败: {}", e)))? {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(())
    }

    async fn open_read_stream(&self, remote_path: &str) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>> {
        let tmp = std::env::temp_dir().join(format!("csync-s3-{}.bin", uuid::Uuid::new_v4()));
        self.download_file(remote_path, &tmp).await?;
        let f = tokio::fs::File::open(&tmp).await?;
        Ok(Box::new(f))
    }

    async fn list_objects(&self, prefix: &str) -> Result<ListObjectsResult> {
        let mut all = Vec::new();
        let mut all_prefixes = Vec::new();
        let mut cont: Option<String> = None;
        loop {
            let (objs, prefixes, truncated, next) = self.client.list_objects(prefix, Some(1000), cont.as_deref()).await?;
            all.extend(objs);
            all_prefixes.extend(prefixes);
            if !truncated { break; }
            cont = next;
            if cont.is_none() { break; }
        }
        Ok(ListObjectsResult { objects: all, prefixes: all_prefixes, truncated: false })
    }

    async fn delete_object(&self, remote_path: &str) -> Result<()> { self.client.delete_object(remote_path).await }
    async fn head_size(&self, remote_path: &str) -> Result<u64> {
        let resp = self.client.head_object(remote_path).await?;
        Ok(resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0))
    }
}

pub struct OssStorage { inner: S3Storage, _region: String }

impl OssStorage {
    pub fn new(cfg: &OssConfig) -> Result<Self> {
        let endpoint = cfg.endpoint.clone()
            .or_else(|| Some(format!("https://oss-{}.aliyuncs.com", cfg.region)));
        let s3_cfg = S3Config {
            name: cfg.name.clone(), region: cfg.region.clone(), bucket: cfg.bucket.clone(),
            access_key: cfg.access_key.clone(), secret_key: cfg.secret_key.clone(),
            endpoint, path_style: cfg.path_style,
        };
        Ok(Self { inner: S3Storage::new(&s3_cfg)?, _region: cfg.region.clone() })
    }
}

#[async_trait]
impl Storage for OssStorage {
    fn kind(&self) -> StorageKind { StorageKind::Oss }
    async fn test_connection(&self) -> Result<TestConnectionResult> { self.inner.test_connection().await }
    async fn upload_file(&self, p: &Path, k: &str) -> Result<()> { self.inner.upload_file(p, k).await }
    async fn upload_stream(&self, s: Box<dyn futures_util::Stream<Item = std::io::Result<Bytes>> + Send + Unpin>, k: &str) -> Result<()> { self.inner.upload_stream(s, k).await }
    async fn download_file(&self, k: &str, p: &Path) -> Result<()> { self.inner.download_file(k, p).await }
    async fn open_read_stream(&self, k: &str) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>> { self.inner.open_read_stream(k).await }
    async fn list_objects(&self, p: &str) -> Result<ListObjectsResult> { self.inner.list_objects(p).await }
    async fn delete_object(&self, k: &str) -> Result<()> { self.inner.delete_object(k).await }
    async fn head_size(&self, k: &str) -> Result<u64> { self.inner.head_size(k).await }
}

pub struct BaiduStorage { client: Arc<crate::netdisk::NetdiskClient> }

impl BaiduStorage {
    pub fn new(client: Arc<crate::netdisk::NetdiskClient>) -> Self { Self { client } }
    fn normalize_dir(remote_path: &str) -> String {
        let p = Path::new(remote_path);
        match p.parent() { Some(par) if !par.as_os_str().is_empty() => par.to_string_lossy().to_string(), _ => "/".to_string() }
    }
}

#[async_trait]
impl Storage for BaiduStorage {
    fn kind(&self) -> StorageKind { StorageKind::Baidu }

    async fn test_connection(&self) -> Result<TestConnectionResult> {
        let started = Instant::now();
        match self.client.verify_bduss().await {
            true => Ok(TestConnectionResult {
                ok: true,
                latency_ms: started.elapsed().as_millis() as u64,
                error: None,
                sample_objects: vec![format!("baidu-user: {}", self.client.uid())],
            }),
            false => Err(CloudSyncError::auth(format!("百度账号 {} 的 BDUSS 已失效", self.client.uid()))),
        }
    }

    async fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<()> {
        let data = tokio::fs::read(local_path).await
            .map_err(|e| CloudSyncError::internal(format!("读取本地文件失败: {}", e)))?;
        let size = data.len() as u64;
        if size > 4 * 1024 * 1024 {
            return Err(CloudSyncError::config(format!("百度单次上传 ≤ 4MB；文件 {} 超出限制", size)));
        }
        let md5_hex = format!("{:x}", md5::compute(&data));
        let block_list = format!("[\"{}\"]", md5_hex);
        let pre = self.client.precreate(remote_path, size, &block_list, "3").await
            .map_err(|e| CloudSyncError::internal(format!("precreate 失败: {}", e)))?;
        if pre.is_rapid_upload() { return Ok(()); }
        let upload_id = pre.uploadid.clone();
        self.client.upload_chunk(remote_path, &upload_id, 0, data, None).await
            .map_err(|e| CloudSyncError::internal(format!("upload_chunk 失败: {}", e)))?;
        self.client.create_file(remote_path, &block_list, &upload_id, size, "0", "3").await
            .map_err(|e| CloudSyncError::internal(format!("create_file 失败: {}", e)))?;
        Ok(())
    }

    async fn upload_stream(&self, _: Box<dyn futures_util::Stream<Item = std::io::Result<Bytes>> + Send + Unpin>, _: &str) -> Result<()> {
        Err(CloudSyncError::config("百度不支持流式上传，请先落盘"))
    }

    async fn download_file(&self, remote_path: &str, local_path: &Path) -> Result<()> {
        let url = self.client.get_download_url(remote_path, 0).await
            .map_err(|e| CloudSyncError::internal(format!("获取下载链接失败: {}", e)))?;
        if let Some(parent) = local_path.parent() { tokio::fs::create_dir_all(parent).await?; }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3600))
            .build()
            .map_err(|e| CloudSyncError::internal(format!("构建 reqwest 失败: {}", e)))?;
        let mut resp = client.get(&url).send().await
            .map_err(|e| CloudSyncError::network(format!("下载请求失败: {}", e)))?;
        if !resp.status().is_success() {
            return Err(CloudSyncError::internal(format!("百度下载 HTTP {}", resp.status())));
        }
        let mut file = tokio::fs::File::create(local_path).await?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| CloudSyncError::network(format!("读取下载流失败: {}", e)))? {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(())
    }

    async fn open_read_stream(&self, remote_path: &str) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>> {
        let tmp = std::env::temp_dir().join(format!("csync-baidu-{}.bin", uuid::Uuid::new_v4()));
        self.download_file(remote_path, &tmp).await?;
        let f = tokio::fs::File::open(&tmp).await?;
        Ok(Box::new(f))
    }

    async fn list_objects(&self, prefix: &str) -> Result<ListObjectsResult> {
        let dir = if prefix.is_empty() { "/" } else { prefix };
        let resp = self.client.get_file_list(dir, 1, 200).await
            .map_err(|e| CloudSyncError::internal(format!("百度列目录失败: {}", e)))?;
        let mut objs = Vec::new();
        let mut prefixes = Vec::new();
        for item in resp.list {
            if item.is_directory() { prefixes.push(item.path); }
            else {
                let last_modified = chrono::DateTime::from_timestamp(item.server_mtime, 0);
                objs.push(ObjectInfo {
                    key: item.path.clone(), size: item.size, last_modified, etag: item.md5.clone(),
                });
            }
        }
        Ok(ListObjectsResult { objects: objs, prefixes, truncated: false })
    }

    async fn delete_object(&self, remote_path: &str) -> Result<()> {
        self.client.delete_files(&[remote_path.to_string()]).await
            .map_err(|e| CloudSyncError::internal(format!("百度删除失败: {}", e)))?;
        Ok(())
    }

    async fn head_size(&self, remote_path: &str) -> Result<u64> {
        let dir = Self::normalize_dir(remote_path);
        let resp = self.client.get_file_list(&dir, 1, 200).await
            .map_err(|e| CloudSyncError::internal(format!("百度查询失败: {}", e)))?;
        let name = Path::new(remote_path).file_name().and_then(|n| n.to_str()).unwrap_or(remote_path);
        for it in resp.list { if it.server_filename == name { return Ok(it.size); } }
        Err(CloudSyncError::not_found(format!("百度未找到: {}", remote_path)))
    }
}

#[async_trait]
pub trait BaiduClientResolver: Send + Sync {
    async fn resolve(&self, owner_uid: u64) -> Option<Arc<crate::netdisk::NetdiskClient>>;
}

pub async fn build_storage(
    cfg: &ConnectionConfig,
    baidu_client_resolver: &dyn BaiduClientResolver,
) -> Result<Arc<dyn Storage>> {
    match cfg {
        ConnectionConfig::S3(c) => Ok(Arc::new(S3Storage::new(c)?)),
        ConnectionConfig::Oss(c) => Ok(Arc::new(OssStorage::new(c)?)),
        ConnectionConfig::Baidu(c) => {
            let client = baidu_client_resolver.resolve(c.owner_uid).await.ok_or_else(|| {
                CloudSyncError::auth(format!("百度账号 {} 未登录或不存在", c.owner_uid))
            })?;
            Ok(Arc::new(BaiduStorage::new(client)))
        }
    }
}