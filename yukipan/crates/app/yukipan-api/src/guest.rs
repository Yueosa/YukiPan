//! 访客空间接口 (文档第 4、7 章): 匿名上传 + 登录管理 (5 端点)。
//!
//! 匿名上传三道按 IP 限流 (次数/时, 字节/时, 字节/天, 超限 429) + 单文件上限
//! + 扩展名白名单 + 两道配额闸; 访客文件带 TTL, 过期由清扫任务删指向
//! (文档第 4 章: 上传 24 小时过期, 响应里必须当场给出过期时间)。

use std::time::Duration;

use axum::Json;
use axum::extract::{Multipart, Query, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use yukipan_fs::LogicalPath;
use yukipan_store::{GuestRef, Space};

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::images::Page;
use crate::util::{MaybeConnectInfo, client_ip, content_length, internal_io, stream_to_tmp, too_many};
use crate::wire::Envelope;
use tracing::{error, warn};

/// 公开 url 前缀 (nginx 直出, 限速档见文档第 5 章)。
const PUBLIC_URL_PREFIX: &str = "/public/guest/";

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(86400);

/// 访客指向视图 (登录管理列表)。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct GuestView {
    pub id: Uuid,
    pub url: String,
    pub orig_name: String,
    pub sha256: String,
    pub size: u64,
    pub source_ip: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// 上传/分享出参: 必须当场带 expires_at (文档第 4 章)。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct GuestUploadResp {
    pub url: String,
    pub orig_name: String,
    pub sha256: String,
    pub size: u64,
    pub expires_at: DateTime<Utc>,
    pub deduped: bool,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct IdReq {
    id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct ShareReq {
    path: String,
}

/// 清空出参。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ClearResp {
    pub cleared: u64,
}

fn guest_view(r: GuestRef) -> GuestView {
    GuestView {
        id: r.id,
        url: format!("{PUBLIC_URL_PREFIX}{}", r.public_name),
        orig_name: r.orig_name,
        sha256: r.sha256,
        size: r.size,
        source_ip: r.source_ip.to_string(),
        created_at: r.created_at,
        expires_at: r.expires_at,
    }
}

/// `POST /api/guest/upload` — 匿名。三道按 IP 限流全过才收。
pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect_info: MaybeConnectInfo,
    mut mp: Multipart,
) -> Result<Json<Envelope<GuestUploadResp>>, ApiError> {
    let ip = client_ip(&headers, connect_info.0);
    let guest_cfg = &state.config.guest;
    let max_file = state.config.quota.guest_max_file;

    // 第一道: 次数/小时。每次尝试都计数 (被拒的也算, 防试探)。
    let cnt_key = format!("guest:cnt:{ip}");
    let n = state.limiter.incr(&cnt_key, HOUR).await;
    if n > guest_cfg.upload_per_hour {
        return Err(too_many(&state.limiter, &cnt_key, "上传太频繁").await);
    }
    // 第二道前置粗检: 声明长度 + 已用量超字节桶就直接拒, 不收 body。
    if let Some(len) = content_length(&headers) {
        let hour_key = format!("guest:bytes:hour:{ip}");
        let used = state.limiter.get(&hour_key).await;
        if used.saturating_add(len) > guest_cfg.upload_bytes_per_hour {
            return Err(too_many(&state.limiter, &hour_key, "本小时上传流量已超上限").await);
        }
        if len > max_file {
            return Err(ApiError::too_large("单文件超过大小上限"));
        }
    }

    let tmp_dir = state.blobs.data_root().join("tmp");
    std::fs::create_dir_all(&tmp_dir).map_err(internal_io)?;
    let tmp = tmp_dir.join(Uuid::new_v4().to_string());
    let filename = match stream_to_tmp(&mut mp, &tmp, Some(max_file)).await {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    let Some(filename) = filename else {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::bad_request("multipart 缺少 file 字段"));
    };
    let Some(ext) = guest_ext(&filename) else {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::bad_request(
            "不支持的文件类型 (图片/pdf/zip/7z/txt/md)",
        ));
    };

    // 第二、三道按真实大小记字节桶 (被拒的流量也入账, 保守口径)。
    let size = std::fs::metadata(&tmp).map_err(internal_io)?.len();
    let hour_key = format!("guest:bytes:hour:{ip}");
    let hour_bytes = state.limiter.incr_by(&hour_key, size, HOUR).await;
    if hour_bytes > guest_cfg.upload_bytes_per_hour {
        let _ = std::fs::remove_file(&tmp);
        return Err(too_many(&state.limiter, &hour_key, "本小时上传流量已超上限").await);
    }
    let day_key = format!("guest:bytes:day:{ip}");
    let day_bytes = state.limiter.incr_by(&day_key, size, DAY).await;
    if day_bytes > guest_cfg.upload_bytes_per_day {
        let _ = std::fs::remove_file(&tmp);
        return Err(too_many(&state.limiter, &day_key, "今日上传流量已超上限").await);
    }

    // 匿名上传记 owner 的 guest 账 (单用户应用即管理员; 见 store::guest 模块文档)。
    let charged = state.blobs.owner_user_id().await?;
    let resp = finish_guest_upload(
        &state, &tmp, &filename, &ext, ip, charged, size,
    )
    .await;
    if resp.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    resp.map(|r| Json(Envelope::ok(r)))
}

/// `GET /api/guest/list` — 要登录, 分页。
pub async fn list(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Envelope<Page<GuestView>>>, ApiError> {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(20).clamp(1, 100);
    let (items, total) = state.blobs.list_guest_refs(page, per_page).await?;
    Ok(Json(Envelope::ok(Page {
        items: items.into_iter().map(guest_view).collect(),
        total,
        page,
        per_page,
    })))
}

/// `POST /api/guest/delete` `{ id }` — 删指向 (要登录)。
pub async fn delete(
    _user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<IdReq>,
) -> Result<Json<Envelope<()>>, ApiError> {
    let Some(r) = state.blobs.delete_guest_ref(req.id).await? else {
        return Err(ApiError::not_found("访客文件不存在"));
    };
    yukipan_store::finish_guest_ref_delete(&state.blobs, &state.store, &r).await;
    Ok(Json(Envelope::ok(())))
}

/// `POST /api/guest/clear` — 一键清空 (要登录)。
pub async fn clear(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Envelope<ClearResp>>, ApiError> {
    let all = state.blobs.list_all_guest_refs().await?;
    let mut cleared = 0;
    for r in all {
        let Some(r) = state.blobs.delete_guest_ref(r.id).await? else {
            continue;
        };
        yukipan_store::finish_guest_ref_delete(&state.blobs, &state.store, &r).await;
        cleared += 1;
    }
    Ok(Json(Envelope::ok(ClearResp { cleared })))
}

/// `POST /api/guest/share` `{ path }` — 把私有路径指到访客空间 (要登录):
/// 不复制字节, 新公开名 + 新指向 (带 TTL) + 按引用加 guest 配额。
/// 登录用户主动分享, 不过按 IP 的匿名限流。
pub async fn share(
    user: AuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    connect_info: MaybeConnectInfo,
    Json(req): Json<ShareReq>,
) -> Result<Json<Envelope<GuestUploadResp>>, ApiError> {
    let uid = user.0.id;
    let ip = client_ip(&headers, connect_info.0);
    let path = LogicalPath::parse(&req.path)?;
    let name = path
        .file_name()
        .ok_or_else(|| ApiError::bad_request("缺少文件路径"))?;
    let Some(ext) = guest_ext(name) else {
        return Err(ApiError::bad_request(
            "不支持的文件类型 (图片/pdf/zip/7z/txt/md)",
        ));
    };
    let Some((sha256, size)) = state.blobs.get_private_ref(uid, &path).await? else {
        return Err(ApiError::not_found("私有路径不存在或不是文件"));
    };
    let r = hang_guest_ref(&state, name, &ext, &sha256, size, ip, uid).await?;
    Ok(Json(Envelope::ok(GuestUploadResp {
        url: format!("{PUBLIC_URL_PREFIX}{}", r.public_name),
        orig_name: r.orig_name,
        sha256: r.sha256,
        size: r.size,
        expires_at: r.expires_at,
        deduped: false,
    })))
}

/// 匿名上传的收编链: 配额闸 → ingest → 记账 → 公开 hardlink → 指向。
async fn finish_guest_upload(
    state: &AppState,
    tmp: &std::path::Path,
    orig_name: &str,
    ext: &str,
    ip: std::net::IpAddr,
    charged: Option<Uuid>,
    size: u64,
) -> Result<GuestUploadResp, ApiError> {
    let limit = state.config.quota.guest_limit;
    let Some(uid) = charged else {
        // 库里还没有任何用户: 指向必须挂账 (charged_to 外键), 无处可记只能拒。
        // 正常部署 bootstrap 已建好管理员, 不会走到这里。
        let _ = std::fs::remove_file(tmp);
        error!("访客上传被拒: 库里没有任何用户, 无法挂配额账");
        return Err(ApiError::Internal("服务未初始化".into()));
    };
    state.store.check_quota(uid, Space::Guest, size, limit).await?;
    state.blobs.check_disk_reserve(state.config.quota.reserve)?;
    let outcome = state.blobs.ingest_blob(tmp, None).await?;
    if let Err(e) = state.store.usage_add(uid, Space::Guest, outcome.size, limit).await {
        rollback_blob(state, &outcome.sha256).await;
        return Err(e.into());
    }
    match insert_guest_link(state, orig_name, ext, &outcome.sha256, outcome.size, ip, uid).await {
        Ok(r) => Ok(GuestUploadResp {
            url: format!("{PUBLIC_URL_PREFIX}{}", r.public_name),
            orig_name: r.orig_name,
            sha256: r.sha256,
            size: r.size,
            expires_at: r.expires_at,
            deduped: outcome.deduped,
        }),
        Err(e) => {
            rollback_usage(state, uid, outcome.size).await;
            rollback_blob(state, &outcome.sha256).await;
            Err(e)
        }
    }
}

/// 挂一条访客指向: 公开 hardlink + 插行 (记账由调用方先做)。
async fn hang_guest_ref(
    state: &AppState,
    orig_name: &str,
    ext: &str,
    sha256: &str,
    size: u64,
    ip: std::net::IpAddr,
    charged: Uuid,
) -> Result<GuestRef, ApiError> {
    let limit = state.config.quota.guest_limit;
    state
        .store
        .usage_add(charged, Space::Guest, size, limit)
        .await?;
    match insert_guest_link(state, orig_name, ext, sha256, size, ip, charged).await {
        Ok(r) => Ok(r),
        Err(e) => {
            rollback_usage(state, charged, size).await;
            rollback_blob(state, sha256).await;
            Err(e)
        }
    }
}

async fn insert_guest_link(
    state: &AppState,
    orig_name: &str,
    ext: &str,
    sha256: &str,
    size: u64,
    ip: std::net::IpAddr,
    charged: Uuid,
) -> Result<GuestRef, ApiError> {
    let public_name = format!("{}.{}", Uuid::new_v4(), ext);
    let target = state
        .blobs
        .data_root()
        .join("public/guest")
        .join(&public_name);
    std::fs::create_dir_all(target.parent().expect("public/guest 必有父")).map_err(internal_io)?;
    std::fs::hard_link(state.blobs.blob_file_path(sha256), &target).map_err(internal_io)?;
    let expires_at = Utc::now() + chrono::Duration::hours(state.config.guest.ttl_hours as i64);
    match state
        .blobs
        .insert_guest_ref(&public_name, orig_name, sha256, size, ip, charged, expires_at)
        .await
    {
        Ok(r) => Ok(r),
        Err(e) => {
            let _ = std::fs::remove_file(&target);
            Err(e.into())
        }
    }
}

/// 访客扩展名白名单 (文档第 6 章): 图床图片 + pdf/zip/7z/txt/md。
/// html/svg/js 不在白名单, 天然被挡 (文档第 5 章)。
fn guest_ext(name: &str) -> Option<String> {
    if !name.contains('.') || name.rfind('.') == Some(0) {
        return None;
    }
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "pdf" | "zip" | "7z" | "txt"
        | "md" => Some(ext),
        _ => None,
    }
}

async fn rollback_usage(state: &AppState, uid: Uuid, size: u64) {
    if let Err(e) = state.store.usage_sub(uid, Space::Guest, size).await {
        warn!("回滚减账失败: {e}");
    }
}

async fn rollback_blob(state: &AppState, sha256: &str) {
    if let Err(e) = state.blobs.delete_blob_if_unreferenced(sha256).await {
        warn!("回滚清 blob 失败: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_ext_whitelist() {
        for ok in ["a.jpg", "a.PNG", "a.pdf", "a.zip", "a.7z", "a.txt", "a.md", "x.y.zip"] {
            assert!(guest_ext(ok).is_some(), "{ok}");
        }
        // html/svg/js 与无扩展名都被挡 (文档第 5、6 章)
        for bad in ["a.html", "a.svg", "a.js", "a.exe", "noext", ".zip", "a."] {
            assert!(guest_ext(bad).is_none(), "{bad}");
        }
    }
}

/// 触真库 + 真 Redis:
/// `YUKIPAN_TEST_DB_URL=postgres://... YUKIPAN_TEST_REDIS_URL=redis://... cargo test -p yukipan-api -- --ignored`
#[cfg(test)]
mod db_tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use tempfile::TempDir;
    use tower::ServiceExt;
    use yukipan_config::Config;
    use yukipan_limit::Limiter;
    use yukipan_store::{BlobStore, Space, Store};

    use super::*;

    struct Fixture {
        _tmp: TempDir,
        app: Router,
        state: AppState,
        pool: PgPool,
    }

    async fn fixture(extra_config: &str) -> Fixture {
        let url = std::env::var("YUKIPAN_TEST_DB_URL").expect("缺少 YUKIPAN_TEST_DB_URL");
        let redis_url =
            std::env::var("YUKIPAN_TEST_REDIS_URL").expect("缺少 YUKIPAN_TEST_REDIS_URL");
        let pool = yukipan_db::connect(&url).await.unwrap();
        yukipan_db::migrate(&pool).await.unwrap();
        let tmp = TempDir::new().unwrap();
        let data_root = tmp.path().join("data");
        for sub in ["blobs", "private", "public/images", "public/guest", "tmp"] {
            std::fs::create_dir_all(data_root.join(sub)).unwrap();
        }
        let config = Config::parse(&format!(
            "database_url = \"{url}\"\n[quota]\nreserve = 0\n{extra_config}"
        ))
        .unwrap();
        let state = AppState {
            store: Store::new(pool.clone()),
            blobs: BlobStore::new(pool.clone(), &data_root),
            limiter: Limiter::connect(&redis_url).await,
            config,
        };
        let app = crate::router(state.clone());
        Fixture {
            _tmp: tmp,
            app,
            state,
            pool,
        }
    }

    async fn make_user(state: &AppState, tag: Uuid, name: &str) -> (Uuid, String) {
        let username = format!("{name}-{tag}");
        let u = state.store.create_user(&username, "pw").await.unwrap();
        (u.id, username)
    }

    async fn login(app: &Router, username: &str, ip: &str) -> String {
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .header("x-real-ip", ip)
                    .body(Body::from(format!(
                        r#"{{"username":"{username}","password":"pw"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        resp.headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        ip: Option<&str>,
        content_type: Option<&str>,
        body: Body,
    ) -> Response {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        if let Some(ip) = ip {
            req = req.header("x-real-ip", ip);
        }
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        app.clone().oneshot(req.body(body).unwrap()).await.unwrap()
    }

    async fn json_body(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn post_json(
        app: &Router,
        uri: &str,
        cookie: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let resp = call(
            app,
            "POST",
            uri,
            cookie,
            None,
            Some("application/json"),
            Body::from(body.to_string()),
        )
        .await;
        let status = resp.status();
        (status, json_body(resp).await)
    }

    fn multipart(filename: &str, content: &[u8]) -> (String, Vec<u8>) {
        let boundary = "TESTBOUNDARY";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    async fn guest_upload(
        app: &Router,
        ip: &str,
        filename: &str,
        content: &[u8],
    ) -> (StatusCode, Value) {
        let (ct, body) = multipart(filename, content);
        let resp = call(app, "POST", "/api/guest/upload", None, Some(ip), Some(&ct), Body::from(body)).await;
        let status = resp.status();
        (status, json_body(resp).await)
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL 与 Redis, 设 YUKIPAN_TEST_DB_URL/YUKIPAN_TEST_REDIS_URL 后加 --ignored 跑"]
    async fn guest_full_flow() {
        let f = fixture("").await;
        let tag = Uuid::new_v4();
        let (_u1, name1) = make_user(&f.state, tag, "g1").await;
        let c1 = login(&f.app, &name1, "10.0.0.1").await;
        // 限流桶在 Redis 里活一小时, 测试 IP 按 run 唯一, 重复跑互不干扰
        let guest_ip = format!("1.2.3.{}", tag.as_u128() % 200 + 1);

        // 匿名上传: url/过期时间当场给出 (文档第 4 章)
        let content = format!("guest doc {tag}").into_bytes();
        let l1 = content.len() as u64;
        let (st, v) = guest_upload(&f.app, &guest_ip, "a.pdf", &content).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["data"]["deduped"], false);
        let url = v["data"]["url"].as_str().unwrap().to_string();
        assert!(url.starts_with("/public/guest/"));
        assert!(url.ends_with(".pdf"));
        let sha = v["data"]["sha256"].as_str().unwrap().to_string();
        let id1 = v["data"]["url"].as_str().unwrap().to_string();
        let expires = v["data"]["expires_at"].as_str().unwrap();
        assert!(!expires.is_empty());
        let public1 = url.trim_start_matches("/public/guest/").to_string();
        assert!(f.state.blobs.data_root().join("public/guest").join(&public1).exists());

        // 白名单: exe 不收 (html/svg/js 也不在白名单)
        let (st, _) = guest_upload(&f.app, &guest_ip, "evil.exe", b"x").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = guest_upload(&f.app, &guest_ip, "evil.html", b"<html>").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // 列表要登录; 登录后可见, 带来源 IP 与过期时间
        let resp = call(&f.app, "GET", "/api/guest/list", None, None, None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let resp = call(&f.app, "GET", "/api/guest/list", Some(&c1), None, None, Body::empty()).await;
        let v = json_body(resp).await;
        let item = v["data"]["items"].as_array().unwrap().iter()
            .find(|i| i["url"] == id1).unwrap().clone();
        assert_eq!(item["source_ip"], guest_ip);
        assert_eq!(item["orig_name"], "a.pdf");
        let gid1 = item["id"].as_str().unwrap().to_string();

        // 匿名上传的账记 owner (dev 库里最早的用户); 读本行的 charged_to 对账
        let g = f.state.blobs.get_guest_ref(gid1.parse().unwrap()).await.unwrap().unwrap();
        let used = f.state.store.usage_get(g.charged_to, Space::Guest).await.unwrap();
        assert!(used >= l1, "owner guest 账本 {used} 应至少含本文件 {l1}");

        // 登录分享: 私有路径 → 访客指向, 不复制字节
        let (ct, body) = multipart("doc.pdf", &content);
        let resp = call(&f.app, "POST", "/api/fs/upload?path=", Some(&c1), None, Some(&ct), Body::from(body)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let (st, v) = post_json(&f.app, "/api/guest/share", Some(&c1), json!({"path": "doc.pdf"})).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert!(v["data"]["url"].as_str().unwrap().starts_with("/public/guest/"));
        assert_eq!(v["data"]["sha256"], sha); // 同一份内容
        let share_id_url = v["data"]["url"].as_str().unwrap().to_string();
        // 引用计数: 匿名 1 + 私有 1 + 分享 1 = 3
        assert_eq!(f.state.blobs.blob_ref_count(&sha).await.unwrap(), 3);
        // 分享不存在的路径 → 404; 不支持的类型 → 400
        let (st, _) = post_json(&f.app, "/api/guest/share", Some(&c1), json!({"path": "missing.pdf"})).await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // 删匿名那条: blob 还在 (还有 2 条指向), 公开文件没了
        let resp = call(&f.app, "POST", "/api/guest/delete", Some(&c1), None, Some("application/json"),
            Body::from(json!({"id": gid1}).to_string())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!f.state.blobs.data_root().join("public/guest").join(&public1).exists());
        assert!(f.state.blobs.blob_file_path(&sha).exists());
        assert_eq!(f.state.blobs.blob_ref_count(&sha).await.unwrap(), 2);

        // 一键清空: 分享那条也被清; 私有指向不受影响, blob 还活着。
        // 列表是全局的 (并行用例也会传), 断言我们的条目消失即可, 不断言全局总数。
        let resp = call(&f.app, "POST", "/api/guest/clear", Some(&c1), None, None, Body::empty()).await;
        let v = json_body(resp).await;
        assert!(v["data"]["cleared"].as_u64().unwrap() >= 1);
        let resp = call(&f.app, "GET", "/api/guest/list", Some(&c1), None, None, Body::empty()).await;
        let v = json_body(resp).await;
        assert!(!v["data"]["items"].as_array().unwrap().iter().any(|i| i["url"] == share_id_url));
        assert!(f.state.blobs.blob_file_path(&sha).exists());
        assert_eq!(f.state.blobs.blob_ref_count(&sha).await.unwrap(), 1);

        // TTL 清扫: 再传一条, 手工把它改成已过期, 触发清扫 → 指向/blob 收尾
        let content2 = format!("guest expire {tag}").into_bytes();
        let (st, v) = guest_upload(&f.app, &guest_ip, "old.zip", &content2).await;
        assert_eq!(st, StatusCode::OK);
        let url2 = v["data"]["url"].as_str().unwrap().to_string();
        let public2 = url2.trim_start_matches("/public/guest/").to_string();
        let sha2 = v["data"]["sha256"].as_str().unwrap().to_string();
        sqlx::query("UPDATE guest_refs SET expires_at = now() - interval '1 hour' WHERE public_name = $1")
            .bind(&public2)
            .execute(&f.pool)
            .await
            .unwrap();
        let swept = yukipan_store::sweep_expired_guests(&f.state.blobs, &f.state.store).await.unwrap();
        assert!(swept >= 1, "应至少扫掉 1 条, 实际 {swept}");
        assert!(f.state.blobs.get_guest_ref_by_name(&public2).await.unwrap().is_none());
        assert!(!f.state.blobs.data_root().join("public/guest").join(&public2).exists());
        assert!(!f.state.blobs.blob_file_path(&sha2).exists());
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL 与 Redis, 设 YUKIPAN_TEST_DB_URL/YUKIPAN_TEST_REDIS_URL 后加 --ignored 跑"]
    async fn guest_rate_limits() {
        let tag = Uuid::new_v4();
        // 次数桶: 每小时 2 次
        let f = fixture("[guest]\nupload_per_hour = 2").await;
        let (_u, name) = make_user(&f.state, tag, "rc").await;
        let _ = login(&f.app, &name, "10.0.0.2").await;
        let ip = format!("11.0.0.{}", tag.as_u128() % 200 + 1);
        let c = format!("rate count {tag}").into_bytes();
        let (st, _) = guest_upload(&f.app, &ip, "a.txt", &c).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = guest_upload(&f.app, &ip, "b.txt", &c).await;
        assert_eq!(st, StatusCode::OK);
        // 第三次触发 429: 带 Retry-After 头与剩余时间消息
        let (ct, body) = multipart("c.txt", &c);
        let resp = call(&f.app, "POST", "/api/guest/upload", None, Some(&ip), Some(&ct), Body::from(body)).await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry = resp.headers().get("retry-after").unwrap().to_str().unwrap();
        let retry_secs: u64 = retry.parse().unwrap();
        assert!(retry_secs > 0 && retry_secs <= 3600, "Retry-After {retry}");
        let v = json_body(resp).await;
        let msg = v["message"].as_str().unwrap();
        assert!(msg.contains("上传太频繁") && (msg.contains("分钟") || msg.contains("秒")), "{msg}");
        // 别的 IP 不受影响
        let other_ip = format!("11.0.1.{}", tag.as_u128() % 200 + 1);
        let (st, _) = guest_upload(&f.app, &other_ip, "d.txt", &c).await;
        assert_eq!(st, StatusCode::OK);

        // 字节桶: 每小时 3000 字节, 两次 2000 第二次爆
        let f2 = fixture("[guest]\nupload_per_hour = 1000\nupload_bytes_per_hour = 3000\nupload_bytes_per_day = 5000").await;
        let (_u2, name2) = make_user(&f2.state, tag, "rb").await;
        let _ = login(&f2.app, &name2, "10.0.0.3").await;
        let ip2 = format!("12.0.0.{}", tag.as_u128() % 200 + 1);
        let big = tag.to_string().repeat(60).into_bytes();
        let (st, _) = guest_upload(&f2.app, &ip2, "x.zip", &big[..2000]).await;
        assert_eq!(st, StatusCode::OK);
        let (ct, body) = multipart("y.zip", &big[..2000]);
        let resp = call(&f2.app, "POST", "/api/guest/upload", None, Some(&ip2), Some(&ct), Body::from(body)).await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.headers().get("retry-after").is_some());
        let v = json_body(resp).await;
        assert!(v["message"].as_str().unwrap().contains("本小时上传流量已超上限"));
        // 天桶也在记; 被时桶拒掉的那次不再消耗天桶 (先过时桶才入天账)
        let day = f2.state.limiter.get(&format!("guest:bytes:day:{ip2}")).await;
        assert_eq!(day, 2000, "天桶 {day}");
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL 与 Redis, 设 YUKIPAN_TEST_DB_URL/YUKIPAN_TEST_REDIS_URL 后加 --ignored 跑"]
    async fn login_bruteforce_protection() {
        let f = fixture("").await;
        let tag = Uuid::new_v4();
        let (_u, name) = make_user(&f.state, tag, "lb").await;
        let ip = format!("13.0.0.{}", tag.as_u128() % 200 + 1);

        async fn try_login(app: &Router, name: &str, password: &str, ip: &str) -> StatusCode {
            app.clone()
                .oneshot(
                    Request::post("/api/auth/login")
                        .header("content-type", "application/json")
                        .header("x-real-ip", ip)
                        .body(Body::from(format!(
                            r#"{{"username":"{name}","password":"{password}"}}"#
                        )))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }

        // 错 3 次后登对: 成功且计数清零
        for _ in 0..3 {
            assert_eq!(try_login(&f.app, &name, "wrong", &ip).await, StatusCode::UNAUTHORIZED);
        }
        assert_eq!(try_login(&f.app, &name, "pw", &ip).await, StatusCode::OK);
        assert_eq!(f.state.limiter.get(&format!("login:fail:{ip}")).await, 0);

        // 连错 10 次后: 第 11 次 (无论对错) 429
        for i in 0..10 {
            assert_eq!(try_login(&f.app, &name, "wrong", &ip).await, StatusCode::UNAUTHORIZED, "第 {i} 次");
        }
        assert_eq!(try_login(&f.app, &name, "wrong", &ip).await, StatusCode::TOO_MANY_REQUESTS);
        // 429 带 Retry-After 与剩余时间 (窗口 10 分钟)
        let resp = f.app.clone().oneshot(
            Request::post("/api/auth/login")
                .header("content-type", "application/json")
                .header("x-real-ip", &ip)
                .body(Body::from(format!(r#"{{"username":"{name}","password":"pw"}}"#)))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry = resp.headers().get("retry-after").unwrap().to_str().unwrap();
        let retry_secs: u64 = retry.parse().unwrap();
        assert!(retry_secs > 0 && retry_secs <= 600, "Retry-After {retry}");
        let v = json_body(resp).await;
        let msg = v["message"].as_str().unwrap();
        assert!(msg.contains("失败次数过多") && msg.contains("分钟后重试"), "{msg}");
        // 清理, 别影响其他用例 (不同 IP 桶, 但保持整洁)
        f.state.limiter.clear(&format!("login:fail:{ip}")).await;
    }
}
