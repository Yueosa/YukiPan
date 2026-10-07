//! 登录/登出/会话查询与鉴权提取器。

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Duration;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use yukipan_store::User;

use crate::AppState;
use crate::error::ApiError;
use crate::wire::Envelope;

/// 会话 cookie 名。
pub const SESSION_COOKIE: &str = "yukipan_session";

/// 登录/会话返回的用户视图。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UserInfo {
    pub id: Uuid,
    pub username: String,
}

impl From<User> for UserInfo {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            username: u.username,
        }
    }
}

/// `POST /api/auth/login` 入参。
#[derive(Debug, Deserialize)]
pub struct LoginReq {
    username: String,
    password: String,
}

/// `POST /api/auth/login` — 成功 Set-Cookie + 用户信息; 失败不区分用户名/密码。
pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginReq>,
) -> Result<(CookieJar, Json<Envelope<UserInfo>>), ApiError> {
    if req.username.is_empty() || req.password.is_empty() {
        return Err(ApiError::bad_request("用户名和密码不能为空"));
    }
    let user = state
        .store
        .verify_login(&req.username, &req.password)
        .await?
        .ok_or_else(|| ApiError::unauthorized("用户名或密码错误"))?;
    let ttl = Duration::hours(state.config.session_ttl_hours as i64);
    let token = state.store.create_session(user.id, ttl).await?;
    // HttpOnly + SameSite=Lax 挡 XSS 取 cookie 与跨站携带; Secure 在 localhost 上浏览器也放行。
    let cookie = Cookie::build((SESSION_COOKIE, token.to_string()))
        .path("/api")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .build();
    Ok((jar.add(cookie), Json(Envelope::ok(user.into()))))
}

/// `POST /api/auth/logout` — 有没有会话都成功, 顺带清 cookie。
pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Json<Envelope<()>>), ApiError> {
    if let Some(token) = jar
        .get(SESSION_COOKIE)
        .and_then(|c| Uuid::parse_str(c.value()).ok())
    {
        state.store.delete_session(token).await?;
    }
    let removal = Cookie::build((SESSION_COOKIE, "")).path("/api").build();
    Ok((jar.remove(removal), Json(Envelope::ok(()))))
}

/// `GET /api/auth/me` — 要登录。
pub async fn me(user: AuthUser) -> Json<Envelope<UserInfo>> {
    Json(Envelope::ok(user.0.into()))
}

/// 鉴权提取器: 无 cookie / 令牌非法 / 会话过期一律 401。
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(SESSION_COOKIE)
            .and_then(|c| Uuid::parse_str(c.value()).ok())
            .ok_or_else(|| ApiError::unauthorized("未登录"))?;
        let user = state
            .store
            .resolve_session(token)
            .await?
            .ok_or_else(|| ApiError::unauthorized("未登录或会话已过期"))?;
        Ok(Self(user))
    }
}
