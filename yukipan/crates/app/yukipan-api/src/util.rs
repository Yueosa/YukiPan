//! 上传共用小件: multipart 流式落 tmp、Content-Length、错误映射 (私有区与图床共用)。

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

use axum::extract::{ConnectInfo, FromRequestParts, Multipart};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use tokio::io::AsyncWriteExt;

use crate::AppState;
use crate::error::ApiError;
use yukipan_limit::Limiter;
use tracing::error;

/// ConnectInfo 的可选版: axum 0.8 的 ConnectInfo 没有 OptionalFromRequestParts,
/// 直接写 Option<ConnectInfo> 不让过 Handler; 包一层, oneshot 测试 (无连接信息)
/// 或非常规部署下取 None 而不是 500。
pub struct MaybeConnectInfo(pub Option<SocketAddr>);

impl FromRequestParts<AppState> for MaybeConnectInfo {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0),
        ))
    }
}

/// 客户端 IP: X-Real-IP 优先, 其次 ConnectInfo, 都没有退回 0.0.0.0
/// (直连无头场景共享一个限流桶)。
///
/// 信任边界: X-Real-IP 只在 nginx 反代之后可信 (nginx 会重写它, 文档第 8 章的
/// 部署形态); 若绕过 nginx 直连后端, 客户端可伪造该头绕过按 IP 限流 — 这是
/// 部署约定兜底的问题, 不在这里防。
pub fn client_ip(headers: &HeaderMap, connect_info: Option<SocketAddr>) -> IpAddr {
    if let Some(ip) = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
    {
        return ip;
    }
    if let Some(addr) = connect_info {
        return addr.ip();
    }
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

/// 收 multipart 的 `file` 字段流式写进 tmp, 返回声明的文件名。
/// `max_bytes` 给上限 (图床单文件 20MB 这档): 写超就中止报 413,
/// 不等收完 — 上传内存与 tmp 占用都有界 (文档第 1、2 章)。
pub async fn stream_to_tmp(
    mp: &mut Multipart,
    tmp: &Path,
    max_bytes: Option<u64>,
) -> Result<Option<String>, ApiError> {
    let mut file = tokio::fs::File::create(tmp).await.map_err(internal_io)?;
    while let Some(mut field) = mp.next_field().await.map_err(multipart_err)? {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().map(str::to_owned);
        let mut written = 0u64;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    written += chunk.len() as u64;
                    if let Some(max) = max_bytes
                        && written > max
                    {
                        return Err(ApiError::too_large("单文件超过大小上限"));
                    }
                    file.write_all(&chunk).await.map_err(internal_io)?;
                }
                Ok(None) => break,
                Err(e) => return Err(multipart_err(e)),
            }
        }
        file.flush().await.map_err(internal_io)?;
        return Ok(Some(
            filename.ok_or_else(|| ApiError::bad_request("file 字段缺少文件名"))?,
        ));
    }
    Ok(None)
}

pub fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

pub fn multipart_err(e: axum::extract::multipart::MultipartError) -> ApiError {
    ApiError::bad_request(format!("multipart 解析失败: {}", e.status()))
}

pub fn internal_io(e: std::io::Error) -> ApiError {
    error!("io error: {e}");
    ApiError::Internal("内部错误".into())
}

/// 429 组装: 从限流桶 TTL 算剩余时间, message 带上, 响应头带 Retry-After。
/// 桶 TTL 拿不到 (降级/桶刚过期) 退回无剩余时间的干消息、不带 Retry-After。
pub async fn too_many(limiter: &Limiter, key: &str, base: &str) -> ApiError {
    match limiter.ttl(key).await {
        Some(d) => {
            let secs = d.as_secs().max(1);
            ApiError::too_many_after(
                format!("{base}, 请 {}后重试", human_remaining(secs)),
                secs,
            )
        }
        None => ApiError::too_many(format!("{base}, 请稍后再试")),
    }
}

/// 剩余时间人性化: ≥1 小时按小时, ≥1 分钟按分钟, 其余按秒; 全部向上取整。
fn human_remaining(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} 小时", secs.div_ceil(3600))
    } else if secs >= 60 {
        format!("{} 分钟", secs.div_ceil(60))
    } else {
        format!("{secs} 秒")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_remaining_units() {
        assert_eq!(human_remaining(1), "1 秒");
        assert_eq!(human_remaining(59), "59 秒");
        assert_eq!(human_remaining(60), "1 分钟");
        assert_eq!(human_remaining(61), "2 分钟");
        assert_eq!(human_remaining(3599), "60 分钟");
        assert_eq!(human_remaining(3600), "1 小时");
        assert_eq!(human_remaining(3700), "2 小时");
        assert_eq!(human_remaining(90000), "25 小时");
    }
}
