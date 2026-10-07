//! YukiPan HTTP 接口: 路由、wire 类型与鉴权/限流中间件。

mod auth;
mod error;
pub mod fs;
pub mod guest;
pub mod images;
mod quota;
mod util;
mod wire;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::{Json, Router};
use yukipan_config::Config;
use yukipan_limit::Limiter;
use yukipan_store::{BlobStore, Store};

pub use auth::AuthUser;
pub use error::ApiError;
pub use wire::Envelope;

/// 路由共享状态。
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub blobs: BlobStore,
    pub limiter: Limiter,
    pub config: Config,
}

/// 全量路由。当前: 探活 + 登录/登出/会话查询 + 私有存储 8 端点。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/quota", get(quota::quota))
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
        // 图床: 公开读
        .route("/api/albums", get(images::list_albums))
        .route("/api/images/list", get(images::list_images))
        .route("/api/tags", get(images::list_tags))
        // 图床: 登录管理
        .route("/api/albums", post(images::create_album))
        .route("/api/albums/delete", post(images::delete_album))
        .route(
            "/api/images/upload",
            post(images::upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/images/instant", post(images::instant))
        .route("/api/images/delete", post(images::delete_image))
        .route("/api/images/tags", post(images::set_tags))
        .route("/api/images/share", post(images::share))
        // 访客: 匿名上传 (限流走 Limiter, 摘默认 body 限制, 50MB 档在 handler 里卡)
        .route(
            "/api/guest/upload",
            post(guest::upload).layer(DefaultBodyLimit::disable()),
        )
        // 访客: 登录管理
        .route("/api/guest/list", get(guest::list))
        .route("/api/guest/delete", post(guest::delete))
        .route("/api/guest/clear", post(guest::clear))
        .route("/api/guest/share", post(guest::share))
        // 访客密钥门: verify 公开, keys 管理要登录
        .route("/api/guest/verify", post(guest::verify))
        .route("/api/guest/keys", get(guest::list_keys).post(guest::create_key))
        .route("/api/guest/keys/revoke", post(guest::revoke_key))
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
            limiter: Limiter::degraded(),
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
    async fn quota_requires_login() {
        let resp = router(test_state())
            .oneshot(Request::get("/api/quota").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// 访客管理端点无 cookie 一律 401 (upload 是匿名口, 不在此列)。
    #[tokio::test]
    async fn guest_admin_routes_require_login() {
        let cases: Vec<(&str, &str, Body)> = vec![
            ("GET", "/api/guest/list", Body::empty()),
            (
                "POST",
                "/api/guest/delete",
                Body::from(r#"{"id":"00000000-0000-0000-0000-000000000000"}"#),
            ),
            ("POST", "/api/guest/clear", Body::empty()),
            ("POST", "/api/guest/share", Body::from(r#"{"path":"a.txt"}"#)),
            ("GET", "/api/guest/keys", Body::empty()),
            ("POST", "/api/guest/keys", Body::from(r#"{"ttl":"1h"}"#)),
            (
                "POST",
                "/api/guest/keys/revoke",
                Body::from(r#"{"id":"00000000-0000-0000-0000-000000000000"}"#),
            ),
        ];
        for (method, uri, body) in cases {
            let resp = router(test_state())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .body(body)
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
    }

    /// 图床管理端点无 cookie 一律 401; 公开端点 (albums/images/tags) 不在此列。
    #[tokio::test]
    async fn image_admin_routes_require_login() {
        let cases: Vec<(&str, &str, Body)> = vec![
            ("POST", "/api/albums", Body::from(r#"{"name":"a"}"#)),
            (
                "POST",
                "/api/albums/delete",
                Body::from(r#"{"id":"00000000-0000-0000-0000-000000000000"}"#),
            ),
            ("POST", "/api/images/upload", Body::empty()),
            (
                "POST",
                "/api/images/instant",
                Body::from(r#"{"name":"a.png","sha256":"x","size":1}"#),
            ),
            (
                "POST",
                "/api/images/delete",
                Body::from(r#"{"id":"00000000-0000-0000-0000-000000000000"}"#),
            ),
            (
                "POST",
                "/api/images/tags",
                Body::from(r#"{"id":"00000000-0000-0000-0000-000000000000","tags":[]}"#),
            ),
            ("POST", "/api/images/share", Body::from(r#"{"path":"a.png"}"#)),
        ];
        for (method, uri, body) in cases {
            let resp = router(test_state())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .body(body)
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
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
