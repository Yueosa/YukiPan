//! YukiPan HTTP 接口: 路由、wire 类型与鉴权/限流中间件。

mod auth;
mod error;
pub mod fs;
mod wire;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::{Json, Router};
use yukipan_config::Config;
use yukipan_store::{BlobStore, Store};

pub use auth::AuthUser;
pub use error::ApiError;
pub use wire::Envelope;

/// 路由共享状态。
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub blobs: BlobStore,
    pub config: Config,
}

/// 全量路由。当前: 探活 + 登录/登出/会话查询 + 私有存储 8 端点。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/fs/list", get(fs::list))
        .route("/api/fs/mkdir", post(fs::mkdir))
        .route("/api/fs/move", post(fs::move_entry))
        .route("/api/fs/delete", post(fs::delete))
        // 私有区无单文件上限 (大文件走这里), 摘掉 axum 默认 2MB body 限制;
        // 大小由配额闸与 nginx client_max_body_size 卡。
        .route(
            "/api/fs/upload",
            post(fs::upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/fs/instant", post(fs::instant))
        .route("/api/fs/download", get(fs::download))
        .route("/api/fs/preview", get(fs::preview))
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
            store: Store::new(pool.clone()),
            blobs: BlobStore::new(pool, "/nonexistent-test-root"),
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

    /// 私有区 8 端点无 cookie 一律 401 (鉴权提取器最先跑, 不碰库不碰盘)。
    #[tokio::test]
    async fn fs_routes_require_login() {
        let cases: Vec<(&str, &str, Body)> = vec![
            ("GET", "/api/fs/list?path=", Body::empty()),
            ("POST", "/api/fs/mkdir", Body::from(r#"{"path":"a"}"#)),
            ("POST", "/api/fs/move", Body::from(r#"{"from":"a","to":"b"}"#)),
            ("POST", "/api/fs/delete", Body::from(r#"{"path":"a"}"#)),
            ("POST", "/api/fs/upload?path=", Body::empty()),
            (
                "POST",
                "/api/fs/instant",
                Body::from(r#"{"path":"","name":"a","sha256":"x","size":1}"#),
            ),
            ("GET", "/api/fs/download?path=a", Body::empty()),
            ("GET", "/api/fs/preview?path=a", Body::empty()),
        ];
        for (method, uri, body) in cases {
            let mut req = Request::builder().method(method).uri(uri);
            if method == "POST" {
                req = req.header("content-type", "application/json");
            }
            let resp = router(test_state())
                .oneshot(req.body(body).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri}"
            );
        }
    }
}
