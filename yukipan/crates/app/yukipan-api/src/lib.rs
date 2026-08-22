//! YukiPan HTTP 接口: 路由、wire 类型与鉴权/限流中间件。

use axum::routing::get;
use axum::{Json, Router};

/// 全量路由。当前只有探活, 其余接口随切片接入 (登录 → 私有区 → 访客 → 图床)。
pub fn router() -> Router {
    Router::new().route("/api/health", get(health))
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
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_ok() {
        let resp = router()
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({ "ok": true }));
    }
}
