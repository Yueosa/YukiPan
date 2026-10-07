//! API 层错误: 状态码 + 统一信封。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use yukipan_store::StoreError;

use crate::wire::Envelope;

/// handler 返回的错误, 统一渲成 `{ success: false, data: null, message }`。
#[derive(Debug)]
pub enum ApiError {
    /// 未登录/会话失效/凭证错误。
    Unauthorized(String),
    /// 入参不合法。
    BadRequest(String),
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
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::UsernameTaken => Self::bad_request("用户名已存在"),
            other => {
                eprintln!("store error: {other}");
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
            Self::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(Envelope::err(message))).into_response()
    }
}
