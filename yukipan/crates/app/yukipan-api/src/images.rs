//! 图床接口 (文档第 3、7 章): 公开读 (相册/墙/标签) + 登录管理 (10 端点)。
//!
//! 组合: store 的相册/图片指向/标签 (库, 列表以库为准) + BlobStore 的 blob 落盘
//! + public/images/ 下的公开 hardlink (server 生成的公开名, 用户输入不进文件名)。
//! 配额走 images 空间 (文档第 6 章: 图床 2G 引用配额 + 20MB 单文件 + 扩展名白名单)。

use axum::Json;
use axum::extract::{Multipart, Query, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use yukipan_fs::LogicalPath;
use yukipan_store::{ImageRef, Space, normalize_tag_name};

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::util::{content_length, internal_io, multipart_err, stream_to_tmp};
use crate::wire::Envelope;

/// 公开 url 前缀 (nginx 直出, 文档第 8 章)。
const PUBLIC_URL_PREFIX: &str = "/public/images/";

/// 相册视图 (公开)。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AlbumView {
    pub id: Uuid,
    pub name: String,
    pub is_default: bool,
    pub image_count: u64,
    pub cover_url: Option<String>,
}

/// 标签视图 (公开)。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct TagView {
    pub id: Uuid,
    pub name: String,
    pub count: u64,
}

/// 图片视图 (公开): 不带 user_id 等隐私字段。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ImageView {
    pub id: Uuid,
    pub url: String,
    pub orig_name: String,
    pub sha256: String,
    pub size: u64,
    pub album_id: Uuid,
    pub album_name: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// 上传/秒传/分享出参: 图片视图 + 是否复用了墙上已有指向。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ImageUploadResp {
    #[serde(flatten)]
    pub image: ImageView,
    pub deduped: bool,
}

/// 分页出参。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    album_id: Option<Uuid>,
    tag: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct NameReq {
    name: String,
}

#[derive(Debug, Deserialize)]
pub struct IdReq {
    id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct InstantReq {
    /// 原名 (展示与扩展名判定的依据)。
    name: String,
    sha256: String,
    size: u64,
    album_id: Option<Uuid>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct TagsReq {
    id: Uuid,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ShareReq {
    path: String,
    album_id: Option<Uuid>,
}

fn image_view(i: ImageRef) -> ImageView {
    ImageView {
        id: i.id,
        url: format!("{PUBLIC_URL_PREFIX}{}", i.public_name),
        orig_name: i.orig_name,
        sha256: i.sha256,
        size: i.size,
        album_id: i.album_id,
        album_name: i.album_name,
        tags: i.tags,
        created_at: i.created_at,
    }
}

/// `GET /api/albums` — 公开, 全部相册带图片数与封面。
pub async fn list_albums(
    State(state): State<AppState>,
) -> Result<Json<Envelope<Vec<AlbumView>>>, ApiError> {
    let albums = state
        .store
        .list_albums()
        .await?
        .into_iter()
        .map(|a| AlbumView {
            id: a.id,
            name: a.name,
            is_default: a.is_default,
            image_count: a.image_count,
            cover_url: a.cover.map(|c| format!("{PUBLIC_URL_PREFIX}{c}")),
        })
        .collect();
    Ok(Json(Envelope::ok(albums)))
}

/// `GET /api/images/list` — 公开, 按 album_id / tag 筛, offset 分页 (per_page 上限 100)。
pub async fn list_images(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Envelope<Page<ImageView>>>, ApiError> {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(20).clamp(1, 100);
    let tag_norm = q.tag.as_deref().map(normalize_tag_name);
    let tag = tag_norm.as_deref().filter(|t| !t.is_empty());
    let (items, total) = state
        .store
        .list_images(q.album_id, tag, page, per_page)
        .await?;
    Ok(Json(Envelope::ok(Page {
        items: items.into_iter().map(image_view).collect(),
        total,
        page,
        per_page,
    })))
}

/// `GET /api/tags` — 公开, 墙上筛选用。
pub async fn list_tags(
    State(state): State<AppState>,
) -> Result<Json<Envelope<Vec<TagView>>>, ApiError> {
    let tags = state
        .store
        .list_tags()
        .await?
        .into_iter()
        .map(|t| TagView {
            id: t.id,
            name: t.name,
            count: t.count,
        })
        .collect();
    Ok(Json(Envelope::ok(tags)))
}

/// `POST /api/albums` `{ name }` — 建相册。
pub async fn create_album(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<NameReq>,
) -> Result<Json<Envelope<AlbumView>>, ApiError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("相册名不能为空"));
    }
    let a = state.store.create_album(user.0.id, name).await?;
    Ok(Json(Envelope::ok(AlbumView {
        id: a.id,
        name: a.name,
        is_default: a.is_default,
        image_count: 0,
        cover_url: None,
    })))
}

/// `POST /api/albums/delete` `{ id }` — 非空册或默认册拒绝。
pub async fn delete_album(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<IdReq>,
) -> Result<Json<Envelope<()>>, ApiError> {
    state.store.delete_album(user.0.id, req.id).await?;
    Ok(Json(Envelope::ok(())))
}

/// `POST /api/images/upload` — multipart `file`, 可选 `album_id` / `tags` (JSON 数组字符串)。
/// 扩展名白名单 + 单文件上限 (流式截断) + 两道配额闸; 同 hash 已在墙直接回旧图
/// (文档第 6 章: 不新建、不加配额)。
pub async fn upload(
    user: AuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    mut mp: Multipart,
) -> Result<Json<Envelope<ImageUploadResp>>, ApiError> {
    let uid = user.0.id;
    let limit = state.config.quota.images_limit;
    let max_file = state.config.quota.images_max_file;

    // 带 multipart 开销的粗检: 超出 上限 + 1MB 余量直接拒 (精确档在流式写入时)。
    if let Some(len) = content_length(&headers)
        && len > max_file + 1024 * 1024
    {
        return Err(ApiError::too_large("单文件超过大小上限"));
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
    let Some(ext) = image_ext(&filename) else {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::bad_request("不是允许的图片格式 (jpg/jpeg/png/gif/webp/avif)"));
    };

    // 可选的相册与标签字段 (multipart 文本字段, 在 file 之后读)。
    let (album_id, tags) = match read_upload_options(&mut mp).await {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };

    let resp = finish_image_upload(&state, uid, &tmp, &filename, &ext, album_id, tags, limit).await;
    if resp.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    resp.map(|r| Json(Envelope::ok(r)))
}

/// `POST /api/images/instant` — 秒传 (文档第 6 章): 同册已有同 hash 回旧图;
/// 否则要求「库里有此 hash 且用户已持任一指向 (私有或图床)」才挂新指向。
pub async fn instant(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<InstantReq>,
) -> Result<Json<Envelope<ImageUploadResp>>, ApiError> {
    let uid = user.0.id;
    if !is_sha256_hex(&req.sha256) {
        return Err(ApiError::bad_request("sha256 不合法"));
    }
    let Some(ext) = image_ext(&req.name) else {
        return Err(ApiError::bad_request("不是允许的图片格式 (jpg/jpeg/png/gif/webp/avif)"));
    };
    let album = resolve_album(&state, uid, req.album_id).await?;

    // 同册已有同 hash → 直接回那张, 不加配额。
    if let Some(existing) = state
        .store
        .find_image_in_album(uid, album, &req.sha256)
        .await?
    {
        return Ok(Json(Envelope::ok(ImageUploadResp {
            image: image_view(existing),
            deduped: true,
        })));
    }

    // 双条件: 库里有 hash 且用户已持指向; 「不存在」与「不让你传」同回 404 (防探测)。
    let actual = state.blobs.blob_size(&req.sha256).await?;
    let owned = state.blobs.user_has_any_ref(uid, &req.sha256).await?;
    let (Some(actual), true) = (actual, owned) else {
        return Err(ApiError::not_found("内容不存在或不可秒传"));
    };
    if req.size != actual {
        return Err(ApiError::bad_request("声明大小与内容不符"));
    }
    let image = hang_image(&state, uid, album, &req.name, &ext, &req.sha256, actual).await?;
    let image = apply_tags(&state, uid, image, req.tags).await?;
    Ok(Json(Envelope::ok(ImageUploadResp {
        image: image_view(image),
        deduped: false,
    })))
}

/// `POST /api/images/delete` `{ id }` — 删指向: 库行 → 公开文件 → 减账 → blob 收尾。
/// 图床列表以库为准 (文档第 6 章), 所以先删库行; 公开文件删失败只留孤儿 hardlink,
/// 不挡墙, 只记日志。
pub async fn delete_image(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<IdReq>,
) -> Result<Json<Envelope<()>>, ApiError> {
    let uid = user.0.id;
    let image = state.store.delete_image(uid, req.id).await?;
    let path = public_image_path(&state, &image.public_name);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("删除公开图文件失败 ({}): {e}", image.public_name);
    }
    if let Err(e) = state
        .store
        .usage_sub(uid, Space::Images, image.size)
        .await
    {
        eprintln!("删图后减账失败 (账本可能漂移): {e}");
    }
    if let Err(e) = state.blobs.delete_blob_if_unreferenced(&image.sha256).await {
        eprintln!("删图后清理 blob 失败 ({}): {e}", image.sha256);
    }
    Ok(Json(Envelope::ok(())))
}

/// `POST /api/images/tags` `{ id, tags }` — 全量覆盖标签 (不存在的规范化名自动建)。
pub async fn set_tags(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<TagsReq>,
) -> Result<Json<Envelope<Vec<String>>>, ApiError> {
    for t in &req.tags {
        if normalize_tag_name(t).is_empty() {
            return Err(ApiError::bad_request("标签名不能为空白"));
        }
    }
    let tags = state.store.set_image_tags(user.0.id, req.id, &req.tags).await?;
    Ok(Json(Envelope::ok(tags)))
}

/// `POST /api/images/share` `{ path, album_id? }` — 把私有路径指到图床:
/// 不复制字节, 新公开名 + 新指向 + 按引用加配额 (文档第 2、6 章)。
pub async fn share(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<ShareReq>,
) -> Result<Json<Envelope<ImageUploadResp>>, ApiError> {
    let uid = user.0.id;
    let path = LogicalPath::parse(&req.path)?;
    let name = path
        .file_name()
        .ok_or_else(|| ApiError::bad_request("缺少文件路径"))?;
    let Some(ext) = image_ext(name) else {
        return Err(ApiError::bad_request("不是允许的图片格式 (jpg/jpeg/png/gif/webp/avif)"));
    };
    let Some((sha256, size)) = state.blobs.get_private_ref(uid, &path).await? else {
        return Err(ApiError::not_found("私有路径不存在或不是文件"));
    };
    let album = resolve_album(&state, uid, req.album_id).await?;
    if let Some(existing) = state.store.find_image_in_album(uid, album, &sha256).await? {
        return Ok(Json(Envelope::ok(ImageUploadResp {
            image: image_view(existing),
            deduped: true,
        })));
    }
    let image = hang_image(&state, uid, album, name, &ext, &sha256, size).await?;
    Ok(Json(Envelope::ok(ImageUploadResp {
        image: image_view(image),
        deduped: false,
    })))
}

/// upload 的收编链: 配额闸 → ingest → 墙上判重 → 记账 → 公开 hardlink → 指向 → 标签。
async fn finish_image_upload(
    state: &AppState,
    uid: Uuid,
    tmp: &std::path::Path,
    orig_name: &str,
    ext: &str,
    album_id: Option<Uuid>,
    tags: Option<Vec<String>>,
    limit: u64,
) -> Result<ImageUploadResp, ApiError> {
    let size = std::fs::metadata(tmp).map_err(internal_io)?.len();
    state.store.check_quota(uid, Space::Images, size, limit).await?;
    state.blobs.check_disk_reserve(state.config.quota.reserve)?;
    let outcome = state.blobs.ingest_blob(tmp, None).await?;

    // 同 hash 已在墙 (任一相册): 直接回旧图, 不新建不加配额 (文档第 6 章)。
    if let Some(existing) = state.store.find_image_by_hash(uid, &outcome.sha256).await? {
        return Ok(ImageUploadResp {
            image: image_view(existing),
            deduped: true,
        });
    }

    let album = resolve_album(state, uid, album_id).await?;
    let image = hang_image(state, uid, album, orig_name, ext, &outcome.sha256, outcome.size).await?;
    let image = apply_tags(state, uid, image, tags).await?;
    Ok(ImageUploadResp {
        image: image_view(image),
        deduped: false,
    })
}

/// 挂一张图上墙: 记账 → 公开 hardlink → 插指向, 步步带反向回滚。
async fn hang_image(
    state: &AppState,
    uid: Uuid,
    album_id: Uuid,
    orig_name: &str,
    ext: &str,
    sha256: &str,
    size: u64,
) -> Result<ImageRef, ApiError> {
    let limit = state.config.quota.images_limit;
    state
        .store
        .usage_add(uid, Space::Images, size, limit)
        .await?;
    // 公开名 server 生成: <uuid>.<ext>, 用户输入不进公开文件名。
    let public_name = format!("{}.{}", Uuid::new_v4(), ext);
    let target = public_image_path(state, &public_name);
    if let Err(e) = std::fs::hard_link(state.blobs.blob_file_path(sha256), &target) {
        rollback_usage(state, uid, size).await;
        rollback_blob(state, sha256).await;
        return Err(internal_io(e));
    }
    match state.store.insert_image(album_id, &public_name, orig_name, sha256).await {
        Ok(image) => Ok(image),
        Err(e) => {
            let _ = std::fs::remove_file(&target);
            rollback_usage(state, uid, size).await;
            rollback_blob(state, sha256).await;
            Err(e.into())
        }
    }
}

/// 打标签失败不遮主流程? 不 — 标签是请求的一部分, 失败整体报错并回滚指向。
async fn apply_tags(
    state: &AppState,
    uid: Uuid,
    image: ImageRef,
    tags: Option<Vec<String>>,
) -> Result<ImageRef, ApiError> {
    let Some(tags) = tags else { return Ok(image) };
    if tags.is_empty() {
        return Ok(image);
    }
    for t in &tags {
        if normalize_tag_name(t).is_empty() {
            return Err(ApiError::bad_request("标签名不能为空白"));
        }
    }
    state.store.set_image_tags(uid, image.id, &tags).await?;
    Ok(state
        .store
        .get_image(image.id)
        .await?
        .expect("刚挂的图必然存在"))
}

/// 相册入参解析: 给了就校验归属, 没给懒建默认册。
async fn resolve_album(state: &AppState, uid: Uuid, album_id: Option<Uuid>) -> Result<Uuid, ApiError> {
    match album_id {
        Some(id) => {
            if state.store.album_owned(uid, id).await? {
                Ok(id)
            } else {
                Err(ApiError::not_found("相册不存在"))
            }
        }
        None => Ok(state.store.ensure_default_album(uid).await?.id),
    }
}

/// 读 file 之后的可选文本字段: album_id (uuid 字符串), tags (JSON 数组字符串)。
async fn read_upload_options(
    mp: &mut Multipart,
) -> Result<(Option<Uuid>, Option<Vec<String>>), ApiError> {
    let mut album_id = None;
    let mut tags = None;
    while let Some(field) = mp.next_field().await.map_err(multipart_err)? {
        match field.name() {
            Some("album_id") => {
                let text = field.text().await.map_err(multipart_err)?;
                album_id = Some(
                    Uuid::parse_str(text.trim())
                        .map_err(|_| ApiError::bad_request("album_id 不合法"))?,
                );
            }
            Some("tags") => {
                let text = field.text().await.map_err(multipart_err)?;
                tags = Some(
                    serde_json::from_str(&text)
                        .map_err(|_| ApiError::bad_request("tags 需为 JSON 数组字符串"))?,
                );
            }
            _ => {}
        }
    }
    Ok((album_id, tags))
}

fn public_image_path(state: &AppState, public_name: &str) -> std::path::PathBuf {
    state
        .blobs
        .data_root()
        .join("public/images")
        .join(public_name)
}

/// 扩展名白名单判定 (文档第 6 章): 按原名后缀小写归一。
/// html/svg 不在白名单, 天然被挡 (文档第 5 章)。
fn image_ext(name: &str) -> Option<String> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    if name.rfind('.') == Some(0) || !name.contains('.') {
        return None; // 无扩展名或纯点文件 (.png 这种没名字的)
    }
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" => Some(ext),
        _ => None,
    }
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

async fn rollback_usage(state: &AppState, uid: Uuid, size: u64) {
    if let Err(e) = state.store.usage_sub(uid, Space::Images, size).await {
        eprintln!("回滚减账失败: {e}");
    }
}

async fn rollback_blob(state: &AppState, sha256: &str) {
    if let Err(e) = state.blobs.delete_blob_if_unreferenced(sha256).await {
        eprintln!("回滚清 blob 失败: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_ext_whitelist() {
        for ok in ["a.jpg", "a.JPEG", "a.png", "a.gif", "a.WEBP", "a.avif", "x.y.png"] {
            assert!(image_ext(ok).is_some(), "{ok}");
        }
        assert_eq!(image_ext("a.JPG").unwrap(), "jpg");
        // html/svg/js 与无扩展名都被挡 (文档第 5、6 章)
        for bad in ["a.html", "a.svg", "a.js", "a.txt", "noext", ".png", "a."] {
            assert!(image_ext(bad).is_none(), "{bad}");
        }
    }
}

/// 触真库全流程: `YUKIPAN_TEST_DB_URL=postgres://... cargo test -p yukipan-api -- --ignored`
#[cfg(test)]
mod db_tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use serde_json::{Value, json};
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
    }

    async fn fixture(extra_config: &str) -> Fixture {
        let url = std::env::var("YUKIPAN_TEST_DB_URL").expect("缺少 YUKIPAN_TEST_DB_URL");
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
            blobs: BlobStore::new(pool, &data_root),
            limiter: Limiter::degraded(),
            config,
        };
        let app = crate::router(state.clone());
        Fixture {
            _tmp: tmp,
            app,
            state,
        }
    }

    async fn make_user(state: &AppState, tag: Uuid, name: &str) -> (Uuid, String) {
        let username = format!("{name}-{tag}");
        let u = state.store.create_user(&username, "pw").await.unwrap();
        (u.id, username)
    }

    async fn login(app: &Router, username: &str) -> String {
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
        content_type: Option<&str>,
        body: Body,
    ) -> Response {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
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

    async fn post_json(app: &Router, uri: &str, cookie: Option<&str>, body: Value) -> (StatusCode, Value) {
        let resp = call(
            app,
            "POST",
            uri,
            cookie,
            Some("application/json"),
            Body::from(body.to_string()),
        )
        .await;
        let status = resp.status();
        (status, json_body(resp).await)
    }

    /// multipart: file 字段在前, 可选文本字段 (album_id/tags) 在后。
    fn multipart(filename: &str, content: &[u8], extras: &[(&str, &str)]) -> (String, Vec<u8>) {
        let boundary = "TESTBOUNDARY";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
        for (name, value) in extras {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            );
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    async fn upload_image(
        app: &Router,
        cookie: &str,
        filename: &str,
        content: &[u8],
        extras: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let (ct, body) = multipart(filename, content, extras);
        let resp = call(app, "POST", "/api/images/upload", Some(cookie), Some(&ct), Body::from(body)).await;
        let status = resp.status();
        (status, json_body(resp).await)
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn images_full_flow() {
        let f = fixture("").await;
        let tag = Uuid::new_v4();
        let (u1, name1) = make_user(&f.state, tag, "i1").await;
        let (_u2, name2) = make_user(&f.state, tag, "i2").await;
        let c1 = login(&f.app, &name1).await;
        let c2 = login(&f.app, &name2).await;

        // 建册
        let (st, v) = post_json(&f.app, "/api/albums", Some(&c1), json!({"name": "风景"})).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let album_scenery = v["data"]["id"].as_str().unwrap().to_string();
        // 重名 → 409
        let (st, _) = post_json(&f.app, "/api/albums", Some(&c1), json!({"name": "风景"})).await;
        assert_eq!(st, StatusCode::CONFLICT);

        // 传图 (不带相册 → 懒建默认册)
        let img1 = format!("first image {tag}").into_bytes();
        let l1 = img1.len() as u64;
        let (st, v) = upload_image(&f.app, &c1, "a.JPG", &img1, &[]).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["data"]["deduped"], false);
        assert_eq!(v["data"]["album_name"], "默认相册");
        assert!(v["data"]["url"].as_str().unwrap().starts_with("/public/images/"));
        assert!(v["data"]["url"].as_str().unwrap().ends_with(".jpg"));
        let sha1 = v["data"]["sha256"].as_str().unwrap().to_string();
        let id1 = v["data"]["id"].as_str().unwrap().to_string();
        let public1 = v["data"]["url"].as_str().unwrap().trim_start_matches("/public/images/").to_string();
        // 盘上公开文件已建
        assert!(f.state.blobs.data_root().join("public/images").join(&public1).exists());

        // 公开端点无 cookie 可用。注意: 同一测试二进制里多个用例并行, 公开列表是
        // 全局的 — 按本用例的封面 url 定位默认册, 断言不依赖全局总数。
        let resp = call(&f.app, "GET", "/api/albums", None, None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = json_body(resp).await;
        let albums = v["data"].as_array().unwrap();
        let default_album = albums
            .iter()
            .find(|a| {
                a["is_default"] == true
                    && a["cover_url"].as_str().unwrap_or("").ends_with(&public1)
            })
            .unwrap();
        assert_eq!(default_album["image_count"], 1);
        let default_album_id = default_album["id"].as_str().unwrap().to_string();
        // 公开视图不泄露 user_id
        assert!(default_album.get("user_id").is_none());

        // 按相册筛 (用户隔离, 不受并行用例干扰)
        let resp = call(&f.app, "GET", &format!("/api/images/list?album_id={default_album_id}"), None, None, Body::empty()).await;
        let v = json_body(resp).await;
        assert_eq!(v["data"]["total"], 1);
        assert_eq!(v["data"]["items"][0]["orig_name"], "a.JPG");
        let resp = call(&f.app, "GET", &format!("/api/images/list?album_id={album_scenery}"), None, None, Body::empty()).await;
        let v = json_body(resp).await;
        assert_eq!(v["data"]["total"], 0);
        // per_page 上限 100
        let resp = call(&f.app, "GET", "/api/images/list?per_page=500", None, None, Body::empty()).await;
        let v = json_body(resp).await;
        assert_eq!(v["data"]["per_page"], 100);

        // 同 hash 重传: 回旧图, 不加配额
        let (st, v) = upload_image(&f.app, &c1, "b.png", &img1, &[]).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["data"]["deduped"], true);
        assert_eq!(v["data"]["id"], id1);
        assert_eq!(f.state.store.usage_get(u1, Space::Images).await.unwrap(), l1);

        // 秒传换册再挂: 新指向, 配额加一份
        let (st, v) = post_json(&f.app, "/api/images/instant", Some(&c1), json!({
            "name": "c.png", "sha256": sha1, "size": l1, "album_id": album_scenery
        })).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["data"]["deduped"], false);
        assert_eq!(v["data"]["album_id"].as_str().unwrap(), album_scenery);
        let id2 = v["data"]["id"].as_str().unwrap().to_string();
        assert_eq!(f.state.store.usage_get(u1, Space::Images).await.unwrap(), 2 * l1);
        // 同册再秒传: 回旧图不加配额
        let (st, v) = post_json(&f.app, "/api/images/instant", Some(&c1), json!({
            "name": "c.png", "sha256": sha1, "size": l1, "album_id": album_scenery
        })).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["data"]["deduped"], true);
        assert_eq!(v["data"]["id"], id2);
        assert_eq!(f.state.store.usage_get(u1, Space::Images).await.unwrap(), 2 * l1);

        // 秒传防猜: 瞎编 hash 404 / 别的用户没指向 404 / 大小不符 400 / 非图扩展名 400。
        // 大小校验只发生在「挂新指向」路径, 所以用空册 (默认册/风景册已有同 hash, 会直接回旧图)。
        let (st, v) = post_json(&f.app, "/api/albums", Some(&c1), json!({"name": "校验"})).await;
        assert_eq!(st, StatusCode::OK);
        let album_check = v["data"]["id"].as_str().unwrap().to_string();
        let bogus = "0".repeat(64);
        for (cookie, body, expect) in [
            (&c1, json!({"name": "x.png", "sha256": bogus, "size": l1}), StatusCode::NOT_FOUND),
            (&c2, json!({"name": "x.png", "sha256": sha1, "size": l1}), StatusCode::NOT_FOUND),
            (&c1, json!({"name": "x.png", "sha256": sha1, "size": 999, "album_id": album_check}), StatusCode::BAD_REQUEST),
            (&c1, json!({"name": "x.svg", "sha256": sha1, "size": l1}), StatusCode::BAD_REQUEST),
        ] {
            let (st, _) = post_json(&f.app, "/api/images/instant", Some(cookie), body).await;
            assert_eq!(st, expect);
        }

        // 非白名单扩展名上传 → 400
        let (st, _) = upload_image(&f.app, &c1, "evil.svg", b"<svg></svg>", &[]).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // 打标签 (规范化去重) → 公开 tags → 按标签筛
        let (st, v) = post_json(&f.app, "/api/images/tags", Some(&c1), json!({
            "id": id1, "tags": ["风景", "  旅行  2024 ", "风景"]
        })).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["data"], json!(["风景", "旅行 2024"]));
        let resp = call(&f.app, "GET", "/api/tags", None, None, Body::empty()).await;
        let v = json_body(resp).await;
        let tags = v["data"].as_array().unwrap();
        assert!(tags.iter().any(|t| t["name"] == "风景" && t["count"].as_u64().unwrap() >= 1));
        // 标签筛选是全局的 (单用户应用), 历史 run 残留图也会命中 — 断言包含本图即可
        let resp = call(&f.app, "GET", "/api/images/list?tag=%E9%A3%8E%E6%99%AF", None, None, Body::empty()).await;
        let v = json_body(resp).await;
        assert!(v["data"]["items"].as_array().unwrap().iter().any(|i| i["id"] == id1));
        // 折叠空白后的规范化名也能筛中
        let resp = call(&f.app, "GET", "/api/images/list?tag=%E6%97%85%E8%A1%8C%20%20%202024", None, None, Body::empty()).await;
        let v = json_body(resp).await;
        assert!(v["data"]["items"].as_array().unwrap().iter().any(|i| i["id"] == id1));

        // 私有区传图再分享到图床
        let img2 = format!("private photo {tag}").into_bytes();
        let l2 = img2.len() as u64;
        let (ct, body) = multipart("photo.png", &img2, &[]);
        let resp = call(&f.app, "POST", "/api/fs/upload?path=", Some(&c1), Some(&ct), Body::from(body)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let (st, v) = post_json(&f.app, "/api/images/share", Some(&c1), json!({"path": "photo.png"})).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["data"]["deduped"], false);
        let id3 = v["data"]["id"].as_str().unwrap().to_string();
        let sha2 = v["data"]["sha256"].as_str().unwrap().to_string();
        // 同册重复分享 → 回旧图
        let (st, v) = post_json(&f.app, "/api/images/share", Some(&c1), json!({"path": "photo.png"})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["data"]["deduped"], true);
        assert_eq!(v["data"]["id"], id3);
        // 分享不存在的路径 → 404; 非图 → 400
        let (st, _) = post_json(&f.app, "/api/images/share", Some(&c1), json!({"path": "missing.png"})).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (ct, body) = multipart("note.txt", format!("text {tag}").as_bytes(), &[]);
        let resp = call(&f.app, "POST", "/api/fs/upload?path=", Some(&c1), Some(&ct), Body::from(body)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let (st, _) = post_json(&f.app, "/api/images/share", Some(&c1), json!({"path": "note.txt"})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        // 账本: C1 两条图床指向 + C2 一条 = 2*l1 + l2
        assert_eq!(f.state.store.usage_get(u1, Space::Images).await.unwrap(), 2 * l1 + l2);

        // 默认册删除 → 400; 非空册 (风景有 instant 图) → 409
        let (st, _) = post_json(&f.app, "/api/albums/delete", Some(&c1), json!({"id": default_album_id})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = post_json(&f.app, "/api/albums/delete", Some(&c1), json!({"id": album_scenery})).await;
        assert_eq!(st, StatusCode::CONFLICT);

        // 删图: 先删默认册那张 C1 → blob C1 还在 (风景册还有指向), 公开文件没了
        let (st, _) = post_json(&f.app, "/api/images/delete", Some(&c1), json!({"id": id1})).await;
        assert_eq!(st, StatusCode::OK);
        assert!(f.state.blobs.blob_file_path(&sha1).exists());
        assert!(!f.state.blobs.data_root().join("public/images").join(&public1).exists());
        assert_eq!(f.state.store.usage_get(u1, Space::Images).await.unwrap(), l1 + l2);
        // 再删风景册那张 → C1 引用归 0, blob 消失
        let (st, _) = post_json(&f.app, "/api/images/delete", Some(&c1), json!({"id": id2})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(f.state.blobs.blob_ref_count(&sha1).await.unwrap(), 0);
        assert!(!f.state.blobs.blob_file_path(&sha1).exists());
        // 空册可删
        let (st, _) = post_json(&f.app, "/api/albums/delete", Some(&c1), json!({"id": album_scenery})).await;
        assert_eq!(st, StatusCode::OK);

        // 删分享图 → blob C2 因私有指向还在而保留
        let (st, _) = post_json(&f.app, "/api/images/delete", Some(&c1), json!({"id": id3})).await;
        assert_eq!(st, StatusCode::OK);
        assert!(f.state.blobs.blob_file_path(&sha2).exists());
        assert_eq!(f.state.store.usage_get(u1, Space::Images).await.unwrap(), 0);
        // 收尾: 删私有文件 → C2 引用归 0 消失
        let (st, _) = post_json(&f.app, "/api/fs/delete", Some(&c1), json!({"path": "photo.png", "recursive": false})).await;
        assert_eq!(st, StatusCode::OK);
        assert!(!f.state.blobs.blob_file_path(&sha2).exists());
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn images_size_and_quota_gates() {
        let f = fixture("images_max_file = 1024\nimages_limit = 1000").await;
        let tag = Uuid::new_v4();
        let (_u, name) = make_user(&f.state, tag, "iq").await;
        let c = login(&f.app, &name).await;

        // 单文件超 1KB: 流式截断 → 413
        let big = vec![b'x'; 2000];
        let (st, _) = upload_image(&f.app, &c, "big.png", &big, &[]).await;
        assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);

        // 900B 过闸; 再来一张 900B 顶爆 1000B 引用配额 → 413
        let a = tag.to_string().repeat(30).into_bytes();
        let (st, v) = upload_image(&f.app, &c, "a.png", &a[..900], &[]).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let b = format!("b{tag}").repeat(30).into_bytes();
        let (st, _) = upload_image(&f.app, &c, "b.png", &b[..900], &[]).await;
        assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
