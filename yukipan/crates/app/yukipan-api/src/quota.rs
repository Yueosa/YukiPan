//! 配额端点 (文档第 7 章): 三个空间的引用配额用量 + 盘上余量, 单位一律字节。

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use yukipan_store::{Space, disk_free};

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::util::internal_io;
use crate::wire::Envelope;

/// 单空间 `{ used, limit }`。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SpaceQuota {
    pub used: u64,
    pub limit: u64,
}

/// `GET /api/quota` 出参。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct QuotaView {
    pub private: SpaceQuota,
    pub images: SpaceQuota,
    pub guest: SpaceQuota,
    pub disk: DiskView,
}

/// 盘上 `{ free, reserve }` (tmp/库/日志不进账本, 看真实余量)。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DiskView {
    pub free: u64,
    pub reserve: u64,
}

/// `GET /api/quota` — 要登录 (文档第 7 章)。
pub async fn quota(
    user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Envelope<QuotaView>>, ApiError> {
    let uid = user.0.id;
    let q = &state.config.quota;
    let (private, images, guest) = tokio::try_join!(
        state.store.usage_get(uid, Space::Private),
        state.store.usage_get(uid, Space::Images),
        state.store.usage_get(uid, Space::Guest),
    )?;
    let free = disk_free(state.blobs.data_root()).map_err(internal_io)?;
    Ok(Json(Envelope::ok(QuotaView {
        private: SpaceQuota {
            used: private,
            limit: q.private_limit,
        },
        images: SpaceQuota {
            used: images,
            limit: q.images_limit,
        },
        guest: SpaceQuota {
            used: guest,
            limit: q.guest_limit,
        },
        disk: DiskView {
            free,
            reserve: q.reserve,
        },
    })))
}

/// 触真库: `YUKIPAN_TEST_DB_URL=postgres://... cargo test -p yukipan-api -- --ignored`
#[cfg(test)]
mod db_tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tempfile::TempDir;
    use tower::ServiceExt;
    use uuid::Uuid;
    use yukipan_config::Config;
    use yukipan_limit::Limiter;
    use yukipan_store::{BlobStore, Store};

    use super::*;

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn quota_shape_and_values() {
        let url = std::env::var("YUKIPAN_TEST_DB_URL").expect("缺少 YUKIPAN_TEST_DB_URL");
        let pool = yukipan_db::connect(&url).await.unwrap();
        yukipan_db::migrate(&pool).await.unwrap();
        let tmp = TempDir::new().unwrap();
        let config = Config::parse(&format!("database_url = \"{url}\"")).unwrap();
        let state = AppState {
            store: Store::new(pool.clone()),
            blobs: BlobStore::new(pool, tmp.path()),
            limiter: Limiter::degraded(),
            config: config.clone(),
        };
        let app = crate::router(state.clone());

        let username = format!("quota-{}", Uuid::new_v4());
        state.store.create_user(&username, "pw").await.unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"username":"{username}","password":"pw"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();

        let resp = app
            .oneshot(
                Request::get("/api/quota")
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["success"], true);
        // 新用户三空间 used 全 0, limit 来自 config 缺省
        for space in ["private", "images", "guest"] {
            assert_eq!(v["data"][space]["used"], 0, "{space}");
        }
        assert_eq!(v["data"]["private"]["limit"], config.quota.private_limit);
        assert_eq!(v["data"]["images"]["limit"], config.quota.images_limit);
        assert_eq!(v["data"]["guest"]["limit"], config.quota.guest_limit);
        assert!(v["data"]["disk"]["free"].as_u64().unwrap() > 0);
        assert_eq!(v["data"]["disk"]["reserve"], config.quota.reserve);
    }
}
