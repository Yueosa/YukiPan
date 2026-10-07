//! API 层错误: 状态码 + 统一信封。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use yukipan_fs::{PathError, ResolveError};
use yukipan_store::StoreError;

use crate::wire::Envelope;

/// handler 返回的错误, 统一渲成 `{ success: false, data: null, message }`。
#[derive(Debug)]
pub enum ApiError {
    /// 未登录/会话失效/凭证错误。
    Unauthorized(String),
    /// 入参不合法 (含路径不合法)。
    BadRequest(String),
    /// 已登录但无权做这件事。
    Forbidden(String),
    /// 资源不存在; 路径穿越也走这档 — 不把真实布局回出去 (文档第 5 章)。
    NotFound(String),
    /// 状态冲突 (目标已存在、目录非空)。
    Conflict(String),
    /// 配额/大小超限。
    PayloadTooLarge(String),
    /// 限流 (短时计数超阈值, 文档第 5 章)。
    TooManyRequests(String),
    /// 内部错误, 细节只进服务端日志, 不回给前端。
    Internal(String),
}

impl ApiError {
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized(message.into())
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::PayloadTooLarge(message.into())
    }

    pub fn too_many(message: impl Into<String>) -> Self {
        Self::TooManyRequests(message.into())
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::UsernameTaken => Self::bad_request("用户名已存在"),
            StoreError::HashMismatch => Self::bad_request("内容哈希与声明不符"),
            StoreError::BlobNotFound => Self::not_found("内容不存在"),
            StoreError::QuotaExceeded => Self::too_large("空间配额不足"),
            StoreError::DiskReserve { .. } => Self::too_large("磁盘可用空间不足"),
            StoreError::AlbumNameTaken => Self::conflict("相册名已存在"),
            StoreError::AlbumNotFound => Self::not_found("相册不存在"),
            StoreError::AlbumNotEmpty => Self::conflict("相册非空, 请先移走其中的图"),
            StoreError::DefaultAlbumForbidden => Self::bad_request("默认相册不能删除"),
            StoreError::ImageNotFound => Self::not_found("图片不存在"),
            other => {
                eprintln!("store error: {other}");
                Self::Internal("内部错误".into())
            }
        }
    }
}

/// 路径规范化失败 = 入参不合法 (含 `..` 穿越, 文档第 5 章不静默折叠)。
impl From<PathError> for ApiError {
    fn from(e: PathError) -> Self {
        Self::bad_request(e.to_string())
    }
}

/// 根内解析失败。注意 Escape 也回 404: 不区分「不存在」与「不许看」,
/// 真实磁盘路径永远不出现在响应里。
impl From<ResolveError> for ApiError {
    fn from(e: ResolveError) -> Self {
        match e {
            ResolveError::NotFound | ResolveError::Escape => Self::not_found("路径不存在"),
            ResolveError::NotADirectory => Self::bad_request("路径中间分量不是目录"),
            ResolveError::NotEmpty => Self::conflict("目录非空, 需要 recursive"),
            ResolveError::RootForbidden => Self::bad_request("根目录不允许该操作"),
            ResolveError::AlreadyExists => Self::conflict("目标已存在"),
            ResolveError::Io(e) => {
                eprintln!("fs io error: {e}");
                Self::Internal("内部错误".into())
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::PayloadTooLarge(m) => (StatusCode::PAYLOAD_TOO_LARGE, m),
            Self::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, m),
            Self::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(Envelope::err(message))).into_response()
    }
}
