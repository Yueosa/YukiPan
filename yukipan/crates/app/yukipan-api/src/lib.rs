//! YukiPan HTTP 接口: 路由、wire 类型与鉴权/限流中间件。

mod auth;
mod error;
mod wire;

use axum::routing::{get, post};
use axum::{Json, Router};
use yukipan_config::Config;
use yukipan_store::Store;

pub use auth::AuthUser;
pub use error::ApiError;
pub use wire::Envelope;

/// 路由共享状态。
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub config: Config,
}

/// 全量路由。登录切片: 探活 + 登录/登出/会话查询。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .with_state(state)
}

/// `GET /api/health` → `{ ok: true }` (文档第 7 章, 不走统一信封)。
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use tower::ServiceExt;

    /// 懒连接池: 不真正建连, 用于不触库的用例 (无 cookie、入参不合法)。
    fn test_state() -> AppState {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/none")
            .expect("懒连接池");
        AppState {
            store: Store::new(pool),
            config: Config::parse(r#"database_url = "postgres://x""#).unwrap(),
        }
    }

    async fn json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_ok() {
        let resp = router(test_state())
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(json(resp).await, serde_json::json!({ "ok": true }));
    }

    #[tokio::test]
    async fn me_requires_login() {
        let resp = router(test_state())
            .oneshot(Request::get("/api/auth/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json(resp).await,
            serde_json::json!({"success": false, "data": null, "message": "未登录"})
        );
    }

    #[tokio::test]
    async fn login_rejects_empty_fields() {
        let resp = router(test_state())
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"username":"","password":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json(resp).await,
            serde_json::json!({"success": false, "data": null, "message": "用户名和密码不能为空"})
        );
    }
}
