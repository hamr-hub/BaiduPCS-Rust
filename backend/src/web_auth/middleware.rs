//! Web 认证中间件模块
//!
//! 实现 Axum 中间件，用于保护需要认证的 API 端点。
//!
//! ## 功能
//! - 从 Header 或 Cookie 提取 Access Token
//! - 验证令牌有效性
//! - 认证绕过逻辑（auth 端点、静态资源、健康检查）
//! - 将认证状态注入请求上下文
//!
//! ## 重要说明
//! 此中间件仅作用于 Web 访问认证，不影响：
//! - 百度二维码登录轮询
//! - 下载/上传进度轮询
//! - WebSocket 实时推送
//! - 所有其他现有功能

use crate::web_auth::state::WebAuthState;
use crate::web_auth::types::{AuthMode, TokenClaims};
use axum::{
    body::Body,
    extract::State,
    http::{header, Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use tracing::debug;

/// Authorization Header 前缀
const BEARER_PREFIX: &str = "Bearer ";

/// Cookie 名称
const ACCESS_TOKEN_COOKIE: &str = "web_auth_access_token";

/// Web 认证专用 HTTP 状态码
/// 使用 419 (Page Expired / Session Expired) 来区分 Web 认证失败和百度账号认证失败
/// 百度账号认证失败使用标准 401，Web 认证失败使用 419
pub const WEB_AUTH_EXPIRED_STATUS: u16 = 419;

/// 认证错误响应
#[derive(Debug, Serialize)]
pub struct AuthErrorResponse {
    pub code: u16,
    pub error: String,
    pub message: String,
}

impl AuthErrorResponse {
    pub fn web_auth_expired(message: &str) -> Self {
        Self {
            code: WEB_AUTH_EXPIRED_STATUS,
            error: "web_auth_expired".to_string(),
            message: message.to_string(),
        }
    }
}

/// 无需认证即可访问的**只读**端点
///
/// 登录页需要据此渲染当前认证状态；仅返回若干布尔值，不含任何凭据。
/// 注意 `PUT /api/v1/web-auth/config`（改认证模式）**不在**此列。
const PUBLIC_READ_PATHS: &[&str] = &[
    "/api/v1/web-auth/status",
    "/api/v1/web-auth/config",
];

/// 无需认证即可访问的**登录类**端点（凭据本身就在请求体中）
const PUBLIC_LOGIN_PATHS: &[&str] = &[
    "/api/v1/web-auth/login",
    "/api/v1/web-auth/refresh",
];

/// WebSocket 路径
///
/// 浏览器无法为 WebSocket 握手设置自定义 Header，因此令牌通过
/// 查询参数 `?token=` 传递，仅此一条路径做特例放行（仍需校验令牌）。
const WS_PATH: &str = "/api/v1/ws";

/// 检查请求是否可匿名访问
///
/// 设计原则：默认拒绝。白名单按「方法 + 精确路径」双重匹配，
/// 其余 `/api/` 下的所有端点（含百度账号、配置、本地文件、加密密钥等）
/// 一律要求已认证会话。绝不能改成前缀匹配——一旦前缀过宽，鉴权会被整体绕过。
fn is_public_request(method: &Method, path: &str) -> bool {
    // 静态资源与 SPA 路由（不属于 /api/ 前缀）公开
    if !path.starts_with("/api/") {
        return true;
    }

    // 健康检查
    if path == "/health" {
        return true;
    }

    match *method {
        // 只读状态查询
        Method::GET | Method::HEAD => PUBLIC_READ_PATHS.contains(&path),
        // 登录 / 刷新
        Method::POST => PUBLIC_LOGIN_PATHS.contains(&path),
        _ => false,
    }
}

/// 从请求中提取 Access Token
///
/// 按优先级尝试：
/// 1. Authorization Header (Bearer token)
/// 2. Cookie (web_auth_access_token)
fn extract_access_token(request: &Request<Body>) -> Option<String> {
    // 1. 尝试从 Authorization Header 提取
    if let Some(auth_header) = request.headers().get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if auth_str.starts_with(BEARER_PREFIX) {
                let token = auth_str[BEARER_PREFIX.len()..].trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    // 2. 尝试从 Cookie 提取
    if let Some(cookie_header) = request.headers().get(header::COOKIE) {
        if let Ok(cookie_str) = cookie_header.to_str() {
            for cookie in cookie_str.split(';') {
                let cookie = cookie.trim();
                if let Some(value) = cookie.strip_prefix(&format!("{}=", ACCESS_TOKEN_COOKIE)) {
                    let token = value.trim();
                    if !token.is_empty() {
                        return Some(token.to_string());
                    }
                }
            }
        }
    }

    // 3. WebSocket 路径：浏览器无法设置握手 Header，令牌走查询参数
    if request.uri().path() == WS_PATH {
        if let Some(query) = request.uri().query() {
            for pair in query.split('&') {
                if let Some(value) = pair.strip_prefix("token=") {
                    let token = urldecode(value);
                    if !token.is_empty() {
                        return Some(token);
                    }
                }
            }
        }
    }

    None
}

/// 最小化 URL 解码（仅处理 `%XX`，避免为一个小功能引入额外依赖）
fn urldecode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex_pair = &input[i + 1..i + 3];
                match u8::from_str_radix(hex_pair, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Web 认证中间件
///
/// 验证请求的认证状态，根据配置的认证模式决定是否允许访问。
///
/// ## 行为
/// - 当认证模式为 `None` 时，所有请求直接通过
/// - 当认证启用时，验证 Access Token
/// - 认证端点、静态资源等路径绕过认证检查
pub async fn web_auth_middleware(
    State(state): State<Arc<WebAuthState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let method = request.method().clone();

    // 白名单请求直接放行（登录/刷新/状态查询/健康检查/静态资源）
    if is_public_request(&method, path) {
        debug!("Auth bypass for path: {} {}", method, path);
        return next.run(request).await;
    }

    // 获取当前认证模式
    let auth_mode = state.get_auth_mode().await;

    // 如果认证未启用，直接通过
    if auth_mode == AuthMode::None {
        debug!("Auth disabled, allowing request: {} {}", method, path);
        return next.run(request).await;
    }

    // 提取 Access Token
    let token = match extract_access_token(&request) {
        Some(t) => t,
        None => {
            debug!("No access token found for: {} {}", method, path);
            return (
                StatusCode::from_u16(WEB_AUTH_EXPIRED_STATUS).unwrap_or(StatusCode::UNAUTHORIZED),
                Json(AuthErrorResponse::web_auth_expired("未提供认证令牌")),
            )
                .into_response();
        }
    };

    // 验证 Access Token
    match state.token_service.verify_access_token(&token) {
        Ok(claims) => {
            debug!(
                "Token verified for: {} {}, jti: {}",
                method, path, claims.jti
            );
            // 将认证信息注入请求扩展
            let mut request = request;
            request
                .extensions_mut()
                .insert(AuthenticatedUser { claims });
            next.run(request).await
        }
        Err(e) => {
            debug!("Token verification failed for: {} {}: {}", method, path, e);
            (
                StatusCode::from_u16(WEB_AUTH_EXPIRED_STATUS).unwrap_or(StatusCode::UNAUTHORIZED),
                Json(AuthErrorResponse::web_auth_expired("令牌无效或已过期")),
            )
                .into_response()
        }
    }
}

/// 已认证用户信息
///
/// 存储在请求扩展中，供下游处理器使用。
/// 可以作为 Axum 提取器直接在处理器中使用。
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// JWT Claims
    pub claims: TokenClaims,
}

impl AuthenticatedUser {
    /// 获取 JWT ID
    pub fn jti(&self) -> &str {
        &self.claims.jti
    }

    /// 获取令牌过期时间
    pub fn expires_at(&self) -> i64 {
        self.claims.exp
    }

    /// 获取令牌签发时间
    pub fn issued_at(&self) -> i64 {
        self.claims.iat
    }

    /// 获取主题（固定为 "web_auth"）
    pub fn subject(&self) -> &str {
        &self.claims.sub
    }
}

/// 可选的已认证用户
///
/// 用于需要检查认证状态但不强制要求认证的处理器。
#[derive(Debug, Clone)]
pub struct OptionalAuthenticatedUser(pub Option<AuthenticatedUser>);

impl OptionalAuthenticatedUser {
    /// 检查是否已认证
    pub fn is_authenticated(&self) -> bool {
        self.0.is_some()
    }

    /// 获取已认证用户（如果存在）
    pub fn user(&self) -> Option<&AuthenticatedUser> {
        self.0.as_ref()
    }
}

// 实现 FromRequestParts 以便作为提取器使用
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

#[axum::async_trait]
impl<S> FromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<AuthErrorResponse>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthenticatedUser>()
            .cloned()
            .ok_or_else(|| {
                (
                    StatusCode::from_u16(WEB_AUTH_EXPIRED_STATUS)
                        .unwrap_or(StatusCode::UNAUTHORIZED),
                    Json(AuthErrorResponse::web_auth_expired("未认证")),
                )
            })
    }
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for OptionalAuthenticatedUser
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(OptionalAuthenticatedUser(
            parts.extensions.get::<AuthenticatedUser>().cloned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_read_endpoints() {
        assert!(is_public_request(&Method::GET, "/health"));
        assert!(is_public_request(&Method::GET, "/api/v1/web-auth/status"));
        assert!(is_public_request(&Method::GET, "/api/v1/web-auth/config"));
    }

    #[test]
    fn test_public_login_endpoints() {
        assert!(is_public_request(&Method::POST, "/api/v1/web-auth/login"));
        assert!(is_public_request(&Method::POST, "/api/v1/web-auth/refresh"));
    }

    /// 关键回归：改认证模式的 PUT 与只读 GET 同路径，写操作必须要求认证
    #[test]
    fn test_update_auth_config_requires_auth() {
        assert!(!is_public_request(&Method::PUT, "/api/v1/web-auth/config"));
        assert!(!is_public_request(&Method::POST, "/api/v1/web-auth/config"));
        assert!(!is_public_request(&Method::DELETE, "/api/v1/web-auth/config"));
        // 近似路径不得命中白名单
        assert!(!is_public_request(&Method::GET, "/api/v1/web-auth/config/x"));
    }

    #[test]
    fn test_static_resources_are_public() {
        for method in [Method::GET, Method::HEAD, Method::POST] {
            assert!(is_public_request(&method, "/"));
            assert!(is_public_request(&method, "/index.html"));
            assert!(is_public_request(&method, "/assets/main.js"));
            assert!(is_public_request(&method, "/favicon.ico"));
            // SPA 前端路由
            assert!(is_public_request(&method, "/settings"));
        }
    }

    /// 公网暴露的回归测试：以下端点一旦出现在白名单里就是全线失守
    #[test]
    fn test_baidu_auth_endpoints_require_auth() {
        // /api/v1/auth/user 会返回 BDUSS 与完整 Cookie 串
        for method in [Method::GET, Method::POST] {
            assert!(!is_public_request(&method, "/api/v1/auth/user"));
            assert!(!is_public_request(&method, "/api/v1/auth/cookie/login"));
            assert!(!is_public_request(&method, "/api/v1/auth/qrcode/generate"));
            assert!(!is_public_request(&method, "/api/v1/auth/qrcode/status"));
            assert!(!is_public_request(&method, "/api/v1/auth/logout"));
        }
    }

    /// 回归测试：web-auth 凭据管理端点必须要求已认证会话
    #[test]
    fn test_web_auth_management_endpoints_require_auth() {
        for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
            assert!(!is_public_request(&method, "/api/v1/web-auth/password/set"));
            assert!(!is_public_request(&method, "/api/v1/web-auth/totp/setup"));
            assert!(!is_public_request(&method, "/api/v1/web-auth/totp/verify"));
            assert!(!is_public_request(&method, "/api/v1/web-auth/totp/disable"));
            assert!(!is_public_request(
                &method,
                "/api/v1/web-auth/recovery-codes/regenerate"
            ));
            assert!(!is_public_request(&method, "/api/v1/web-auth/logout"));
        }
    }

    /// 中间件挂在顶层 Router 上，`uri().path()` 恒为绝对路径，
    /// 因此这里只断言 `/api/v1/...` 形式（`/files` 这类相对路径是 SPA 路由，属静态资源）
    #[test]
    fn test_api_paths_require_auth() {
        for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
            assert!(!is_public_request(&method, "/api/v1/files"));
            assert!(!is_public_request(&method, "/api/v1/downloads"));
            assert!(!is_public_request(&method, "/api/v1/uploads"));
            assert!(!is_public_request(&method, "/api/v1/config"));
            assert!(!is_public_request(&method, "/api/v1/autobackup/configs"));
            // 旧版本遗漏在 api_relative_paths 里的敏感前缀
            assert!(!is_public_request(&method, "/api/v1/accounts/list"));
            assert!(!is_public_request(&method, "/api/v1/shares"));
            assert!(!is_public_request(&method, "/api/v1/cloud-sync/connections"));
            assert!(!is_public_request(&method, "/api/v1/local-files"));
            assert!(!is_public_request(&method, "/api/v1/proxy/status"));
            assert!(!is_public_request(&method, "/api/v1/encryption/export-keys"));
            assert!(!is_public_request(&method, "/api/v1/fs/list"));
            // WebSocket 同样需要令牌（通过查询参数传递）
            assert!(!is_public_request(&method, "/api/v1/ws"));
        }
    }

    #[test]
    fn test_extract_token_from_ws_query() {
        let request = Request::builder()
            .uri("/api/v1/ws?token=ws_token_789")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_access_token(&request), Some("ws_token_789".to_string()));
    }

    #[test]
    fn test_extract_token_from_ws_query_urlencoded() {
        let request = Request::builder()
            .uri("/api/v1/ws?token=a%2Bb%2Fc%3D&other=1")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_access_token(&request), Some("a+b/c=".to_string()));
    }

    /// 非 WebSocket 路径上的 token 查询参数必须被忽略（避免令牌出现在日志/Referer 里）
    #[test]
    fn test_query_token_ignored_on_other_paths() {
        let request = Request::builder()
            .uri("/api/v1/files?token=leaked_token")
            .body(Body::empty())
            .unwrap();
        assert!(extract_access_token(&request).is_none());
    }

    #[test]
    fn test_extract_access_token_from_header() {
        use axum::http::Request;

        let request = Request::builder()
            .uri("/api/v1/files")
            .header(header::AUTHORIZATION, "Bearer test_token_123")
            .body(Body::empty())
            .unwrap();

        let token = extract_access_token(&request);
        assert_eq!(token, Some("test_token_123".to_string()));
    }

    #[test]
    fn test_extract_access_token_from_cookie() {
        use axum::http::Request;

        let request = Request::builder()
            .uri("/api/v1/files")
            .header(
                header::COOKIE,
                "web_auth_access_token=cookie_token_456; other=value",
            )
            .body(Body::empty())
            .unwrap();

        let token = extract_access_token(&request);
        assert_eq!(token, Some("cookie_token_456".to_string()));
    }

    #[test]
    fn test_extract_access_token_header_priority() {
        use axum::http::Request;

        // Header should take priority over Cookie
        let request = Request::builder()
            .uri("/api/v1/files")
            .header(header::AUTHORIZATION, "Bearer header_token")
            .header(header::COOKIE, "web_auth_access_token=cookie_token")
            .body(Body::empty())
            .unwrap();

        let token = extract_access_token(&request);
        assert_eq!(token, Some("header_token".to_string()));
    }

    #[test]
    fn test_extract_access_token_none() {
        use axum::http::Request;

        let request = Request::builder()
            .uri("/api/v1/files")
            .body(Body::empty())
            .unwrap();

        let token = extract_access_token(&request);
        assert!(token.is_none());
    }

    #[test]
    fn test_extract_access_token_invalid_bearer() {
        use axum::http::Request;

        // Missing "Bearer " prefix
        let request = Request::builder()
            .uri("/api/v1/files")
            .header(header::AUTHORIZATION, "token_without_bearer")
            .body(Body::empty())
            .unwrap();

        let token = extract_access_token(&request);
        assert!(token.is_none());
    }

    #[test]
    fn test_extract_access_token_empty_bearer() {
        use axum::http::Request;

        let request = Request::builder()
            .uri("/api/v1/files")
            .header(header::AUTHORIZATION, "Bearer ")
            .body(Body::empty())
            .unwrap();

        let token = extract_access_token(&request);
        assert!(token.is_none());
    }
}
