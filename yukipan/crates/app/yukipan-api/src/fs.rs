//! 私有存储接口 (文档第 7 章): 8 个端点, 全部要登录。
//!
//! 组合两层底座: fs 目录树 (唯一真源是磁盘, 目录以文件系统为准) +
//! store 指向/账本 (private_refs 落盘指向, usage 按引用计账)。
//! 防护 (文档第 5 章): 所有 path 过 LogicalPath, 响应不含真实磁盘路径,
//! 上传流式落 tmp 不整读内存。

use std::path::PathBuf;

use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use yukipan_fs::{DataRoot, LogicalPath};
use yukipan_store::Space;

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::wire::Envelope;
use crate::AppState;

/// `GET /api/fs/list` 出参条目。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
}

/// 上传/秒传出参 (文档第 7 章: 逻辑路径、大小、hash、是否秒传)。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UploadResp {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub deduped: bool,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PathReq {
    path: String,
}

#[derive(Debug, Deserialize)]
pub struct MoveReq {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteReq {
    path: String,
    recursive: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    /// 目标目录 (文件名取 multipart 里的 file_name)。
    path: Option<String>,
    sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InstantReq {
    /// 目标目录。
    path: String,
    /// 文件名 (单分量)。
    name: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    path: Option<String>,
}

/// `GET /api/fs/list?path=` — 不递归, 目录在前。
pub async fn list(
    user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Envelope<Vec<FsEntry>>>, ApiError> {
    let path = parse_path(q.path.as_deref())?;
    let root = user_root(&state, user.0.id)?;
    let entries = root
        .list_dir(&path)?
        .into_iter()
        .map(|e| FsEntry {
            name: e.name,
            is_dir: e.is_dir,
            size: e.size,
            modified: e.modified.map(DateTime::from),
        })
        .collect();
    Ok(Json(Envelope::ok(entries)))
}

/// `POST /api/fs/mkdir` `{ path }` — 按层层创建。
pub async fn mkdir(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<PathReq>,
) -> Result<Json<Envelope<()>>, ApiError> {
    let path = LogicalPath::parse(&req.path)?;
    user_root(&state, user.0.id)?.create_dir_all(&path)?;
    Ok(Json(Envelope::ok(())))
}

/// `POST /api/fs/move` `{ from, to }` — to 含新名字, 中间目录不代建。
///
/// 先动盘上 (真源), 再把指向路径同步过去: from 是文件改一行, 是目录
/// 其下所有指向按前缀平移 (store::rename_private_refs)。若指向同步失败,
/// 盘上已移动完成, 报错并留日志 — 盘上是真源, 指向可对账重建。
pub async fn move_entry(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<MoveReq>,
) -> Result<Json<Envelope<()>>, ApiError> {
    let from = LogicalPath::parse(&req.from)?;
    let to = LogicalPath::parse(&req.to)?;
    user_root(&state, user.0.id)?.move_entry(&from, &to)?;
    state
        .blobs
        .rename_private_refs(user.0.id, &from, &to)
        .await?;
    Ok(Json(Envelope::ok(())))
}

/// `POST /api/fs/delete` `{ path, recursive }` — 文件与目录统一走前缀语义:
/// 先删盘上 (真源), 再删指向、减账、引用计数归 0 的 blob 一并清掉 (文档第 6 章)。
///
/// 收尾步骤失败 (账本/指向/blob) 只记日志不回滚盘上删除 — 盘上是用户看到的真相,
/// 残留指向会让 blob 多活一会, 由后续 TTL 清扫兜底。
pub async fn delete(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<DeleteReq>,
) -> Result<Json<Envelope<()>>, ApiError> {
    let path = LogicalPath::parse(&req.path)?;
    let recursive = req.recursive.unwrap_or(false);
    let uid = user.0.id;
    // 先取出受影响的指向 (删盘前取, 免得对不上账)。
    let refs = state.blobs.list_private_refs_under(uid, &path).await?;
    user_root(&state, uid)?.remove_entry(&path, recursive)?;
    state.blobs.delete_private_refs_under(uid, &path).await?;
    let total: u64 = refs.iter().map(|r| r.size).sum();
    if total > 0
        && let Err(e) = state.store.usage_sub(uid, Space::Private, total).await
    {
        eprintln!("删除后减账失败 (账本可能漂移): {e}");
    }
    let mut hashes: Vec<&str> = refs.iter().map(|r| r.sha256.as_str()).collect();
    hashes.sort_unstable();
    hashes.dedup();
    for sha in hashes {
        if let Err(e) = state.blobs.delete_blob_if_unreferenced(sha).await {
            eprintln!("删除后清理 blob 失败 ({sha}): {e}");
        }
    }
    Ok(Json(Envelope::ok(())))
}

/// `POST /api/fs/upload?path=&sha256=` — multipart `file` 流式收, 先落 tmp 再收编。
///
/// 顺序: Content-Length 预检 → 流式落 tmp → 两道配额闸 (引用配额 + 磁盘余量,
/// 文档第 6 章) → ingest (算 hash/去重) → 记账 → 建 hardlink → 写指向。
/// 任一步失败都回滚已做的部分, tmp 由 ingest 负责清 (成功失败都清)。
/// 目标已存在按 409 拒绝 — 文档未明说覆盖语义, 选不覆盖 (防误删已有文件)。
pub async fn upload(
    user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    mut mp: Multipart,
) -> Result<Json<Envelope<UploadResp>>, ApiError> {
    let dir = parse_path(q.path.as_deref())?;
    let uid = user.0.id;
    let limit = state.config.quota.private_limit;

    // 有 Content-Length 就先挡一刀, 不必收完才发现超限。
    if let Some(len) = content_length(&headers) {
        state.store.check_quota(uid, Space::Private, len, limit).await?;
    }

    // 流式落 tmp (文档第 2 章: 上传不要把整文件读进后端内存)。
    let tmp_dir = state.blobs.data_root().join("tmp");
    std::fs::create_dir_all(&tmp_dir).map_err(internal_io)?;
    let tmp = tmp_dir.join(Uuid::new_v4().to_string());
    let filename = match stream_to_tmp(&mut mp, &tmp).await {
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
    let logical = match join_logical(&dir, &filename) {
        Ok(l) => l,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };

    let resp = finish_upload(&state, uid, &tmp, &logical, q.sha256.as_deref(), limit).await;
    if resp.is_err() {
        // ingest 内部已清 tmp; 若失败发生在 ingest 之前 (配额闸), 这里补一刀。
        let _ = std::fs::remove_file(&tmp);
    }
    resp.map(|r| Json(Envelope::ok(r)))
}

/// `POST /api/fs/instant` `{ path, name, sha256, size }` — 秒传 (文档第 6 章):
/// 仅当库里有此 hash 且该用户已持有至少一条指向该 hash 的私有指向才放行
/// (只知道 hash 不能秒传, 防猜到私有文件 hash 挂出来)。
/// 「不存在」与「不让你传」回同一个 404, 不区分, 免得变成探测口。
pub async fn instant(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<InstantReq>,
) -> Result<Json<Envelope<UploadResp>>, ApiError> {
    let uid = user.0.id;
    if !is_sha256_hex(&req.sha256) {
        return Err(ApiError::bad_request("sha256 不合法"));
    }
    let dir = LogicalPath::parse(&req.path)?;
    let logical = join_logical(&dir, &req.name)?;

    let actual = state.blobs.blob_size(&req.sha256).await?;
    let owned = state.blobs.user_has_private_ref(uid, &req.sha256).await?;
    let (Some(actual), true) = (actual, owned) else {
        return Err(ApiError::not_found("内容不存在或不可秒传"));
    };
    if req.size != actual {
        return Err(ApiError::bad_request("声明大小与内容不符"));
    }

    let limit = state.config.quota.private_limit;
    state.store.check_quota(uid, Space::Private, actual, limit).await?;
    // 秒传不写新字节但仍走引用配额 (文档第 6 章)。
    state
        .store
        .usage_add(uid, Space::Private, actual, limit)
        .await?;

    let root = user_root(&state, uid)?;
    if let Err(e) = root.link_file(&logical, &state.blobs.blob_file_path(&req.sha256)) {
        rollback_usage(&state, uid, actual).await;
        return Err(e.into());
    }
    if let Err(e) = state.blobs.add_private_ref(uid, &logical, &req.sha256).await {
        let _ = root.remove_entry(&logical, false);
        rollback_usage(&state, uid, actual).await;
        return Err(e.into());
    }
    Ok(Json(Envelope::ok(UploadResp {
        path: logical.to_string(),
        size: actual,
        sha256: req.sha256,
        deduped: true,
    })))
}

/// `GET /api/fs/download?path=` — 鉴权后内部重定向, 附件下载 (文档第 7 章)。
pub async fn download(
    user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<FileQuery>,
) -> Result<Response, ApiError> {
    serve_file(&state, user.0.id, q.path.as_deref(), false)
}

/// `GET /api/fs/preview?path=` — 页内预览。Content-Type 按扩展名安全映射:
/// 图片/pdf 给真类型, 其余 (含 html/svg/js) 一律 text/plain + nosniff,
/// 浏览器只当文本看, 不当页面执行 (文档第 5 章)。
pub async fn preview(
    user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<FileQuery>,
) -> Result<Response, ApiError> {
    serve_file(&state, user.0.id, q.path.as_deref(), true)
}

/// download/preview 共用: 鉴权过 → X-Accel-Redirect 到 nginx 的 /protected/,
/// body 由 nginx 直出, 不过后端内存 (文档第 2、8 章)。
fn serve_file(
    state: &AppState,
    uid: Uuid,
    raw_path: Option<&str>,
    inline: bool,
) -> Result<Response, ApiError> {
    let path = parse_path(raw_path)?;
    if path.is_root() {
        return Err(ApiError::bad_request("缺少文件路径"));
    }
    let root = user_root(state, uid)?;
    let abs = root.resolve_existing(&path)?;
    let meta = std::fs::metadata(&abs).map_err(|e| {
        eprintln!("读文件元数据失败: {e}");
        ApiError::Internal("内部错误".into())
    })?;
    if meta.is_dir() {
        return Err(ApiError::bad_request("不能下载目录"));
    }
    let name = path.file_name().expect("非根路径必有文件名");
    // 路径逐分量百分号编码: 中文等非 ASCII 名字不能直接进响应头。
    let redirect = format!("/protected/{uid}/{}", encode_logical_path(&path));
    let (content_type, disposition) = if inline {
        (preview_content_type(name), "inline".to_string())
    } else {
        ("application/octet-stream", content_disposition("attachment", name))
    };
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header("X-Accel-Redirect", redirect)
        .header("Content-Disposition", disposition);
    if inline {
        builder = builder.header("X-Content-Type-Options", "nosniff");
    }
    Ok(builder.body(Body::empty()).expect("响应构造失败"))
}

/// ingest 之后的收尾链: 记账 → 建链接 → 写指向, 步步带反向回滚。
async fn finish_upload(
    state: &AppState,
    uid: Uuid,
    tmp: &PathBuf,
    logical: &LogicalPath,
    expected_sha: Option<&str>,
    limit: u64,
) -> Result<UploadResp, ApiError> {
    // 两道闸的第二道在收完后按真实大小复核 (第一道是 Content-Length 预检)。
    let size = std::fs::metadata(tmp).map_err(internal_io)?.len();
    state.store.check_quota(uid, Space::Private, size, limit).await?;
    state
        .blobs
        .check_disk_reserve(state.config.quota.reserve)?;

    let outcome = state.blobs.ingest_blob(tmp, expected_sha).await?;

    // 秒传省盘不省账 (文档第 6 章: 去重省磁盘, 账本按引用走)。
    if let Err(e) = state
        .store
        .usage_add(uid, Space::Private, outcome.size, limit)
        .await
    {
        rollback_blob(state, &outcome.sha256).await;
        return Err(e.into());
    }
    let root = user_root(state, uid)?;
    if let Err(e) = root.link_file(logical, &state.blobs.blob_file_path(&outcome.sha256)) {
        rollback_usage(state, uid, outcome.size).await;
        rollback_blob(state, &outcome.sha256).await;
        return Err(e.into());
    }
    if let Err(e) = state.blobs.add_private_ref(uid, logical, &outcome.sha256).await {
        let _ = root.remove_entry(logical, false);
        rollback_usage(state, uid, outcome.size).await;
        rollback_blob(state, &outcome.sha256).await;
        return Err(e.into());
    }
    Ok(UploadResp {
        path: logical.to_string(),
        size: outcome.size,
        sha256: outcome.sha256,
        deduped: outcome.deduped,
    })
}

/// 收 multipart 的 `file` 字段流式写进 tmp, 返回声明的文件名。
async fn stream_to_tmp(mp: &mut Multipart, tmp: &PathBuf) -> Result<Option<String>, ApiError> {
    let mut file = tokio::fs::File::create(tmp).await.map_err(internal_io)?;
    while let Some(mut field) = mp.next_field().await.map_err(multipart_err)? {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().map(str::to_owned);
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    file.write_all(&chunk).await.map_err(internal_io)?;
                }
                Ok(None) => break,
                Err(e) => return Err(multipart_err(e)),
            }
        }
        file.flush().await.map_err(internal_io)?;
        return Ok(Some(filename.ok_or_else(|| ApiError::bad_request("file 字段缺少文件名"))?));
    }
    Ok(None)
}

/// 每用户一个私有根, 首次访问懒建 (避免启动时扫用户表建目录)。
fn user_root(state: &AppState, uid: Uuid) -> Result<DataRoot, ApiError> {
    let dir = state
        .blobs
        .data_root()
        .join("private")
        .join(uid.to_string());
    std::fs::create_dir_all(&dir).map_err(internal_io)?;
    Ok(DataRoot::new(dir)?)
}

/// path 入参统一入口: 缺省为根, 其余过 LogicalPath 规范化 (`..`/反斜杠/控制字符全拒)。
fn parse_path(raw: Option<&str>) -> Result<LogicalPath, ApiError> {
    Ok(LogicalPath::parse(raw.unwrap_or(""))?)
}

/// 目录 + 单分量文件名 → 完整逻辑路径。名字含分隔符/`..` 一律拒。
fn join_logical(dir: &LogicalPath, name: &str) -> Result<LogicalPath, ApiError> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(ApiError::bad_request("文件名不合法"));
    }
    let full = if dir.is_root() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    };
    Ok(LogicalPath::parse(&full)?)
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

/// 预览 Content-Type 安全映射 (文档第 5 章): 只有图片与 pdf 给真类型。
fn preview_content_type(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "pdf" => "application/pdf",
        _ => "text/plain; charset=utf-8",
    }
}

/// RFC 5987 Content-Disposition: ASCII 兜底名 + filename* 原名。
fn content_disposition(kind: &str, name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{}", pct_encode(name))
}

/// 逻辑路径 → URI 路径: 逐分量百分号编码, `/` 保留。
fn encode_logical_path(path: &LogicalPath) -> String {
    path.components()
        .map(pct_encode)
        .collect::<Vec<_>>()
        .join("/")
}

/// 百分号编码, 只保留 RFC 3986 unreserved。
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 减账回滚: 失败只记日志 (账本漂移由对账兜底, 不遮主错误)。
async fn rollback_usage(state: &AppState, uid: Uuid, size: u64) {
    if let Err(e) = state.store.usage_sub(uid, Space::Private, size).await {
        eprintln!("回滚减账失败: {e}");
    }
}

/// 清 blob 回滚: 仅当没有任何指向时才真删 (新建失败场景下它必然无指向;
/// deduped 场景下还有别人的指向, delete_blob_if_unreferenced 自己会判断)。
async fn rollback_blob(state: &AppState, sha256: &str) {
    if let Err(e) = state.blobs.delete_blob_if_unreferenced(sha256).await {
        eprintln!("回滚清 blob 失败: {e}");
    }
}

fn multipart_err(e: axum::extract::multipart::MultipartError) -> ApiError {
    ApiError::bad_request(format!("multipart 解析失败: {}", e.status()))
}

fn internal_io(e: std::io::Error) -> ApiError {
    eprintln!("io error: {e}");
    ApiError::Internal("内部错误".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_logical_validates_name() {
        let root = LogicalPath::root();
        assert_eq!(join_logical(&root, "a.txt").unwrap().as_str(), "a.txt");
        let dir = LogicalPath::parse("docs").unwrap();
        assert_eq!(join_logical(&dir, "a.txt").unwrap().as_str(), "docs/a.txt");
        for bad in ["", "a/b", "a\\b", ".", ".."] {
            assert!(join_logical(&dir, bad).is_err(), "{bad}");
        }
        assert!(join_logical(&dir, "a\nb").is_err());
    }

    #[test]
    fn preview_content_type_mapping() {
        assert_eq!(preview_content_type("a.JPG"), "image/jpeg");
        assert_eq!(preview_content_type("a.png"), "image/png");
        assert_eq!(preview_content_type("a.avif"), "image/avif");
        assert_eq!(preview_content_type("a.pdf"), "application/pdf");
        // html/svg/js 一律纯文本 (文档第 5 章)
        for n in ["a.html", "a.svg", "a.js", "a.txt", "noext"] {
            assert_eq!(preview_content_type(n), "text/plain; charset=utf-8", "{n}");
        }
    }

    #[test]
    fn pct_encode_rules() {
        assert_eq!(pct_encode("a-b.txt"), "a-b.txt");
        assert_eq!(pct_encode("中文.txt"), "%E4%B8%AD%E6%96%87.txt");
        assert_eq!(pct_encode("a b"), "a%20b");
        assert_eq!(pct_encode("a/b"), "a%2Fb");
    }

    #[test]
    fn content_disposition_encodes() {
        let d = content_disposition("attachment", "报告 1.txt");
        assert!(d.starts_with("attachment; filename=\"__ 1.txt\"; filename*=UTF-8''"));
        assert!(d.contains("%E6%8A%A5%E5%91%8A%201.txt"));
    }

    #[test]
    fn encode_path_keeps_slashes() {
        let p = LogicalPath::parse("docs/中文/a.txt").unwrap();
        assert_eq!(encode_logical_path(&p), "docs/%E4%B8%AD%E6%96%87/a.txt");
    }
}

/// 触真库全流程: `YUKIPAN_TEST_DB_URL=postgres://... cargo test -p yukipan-api -- --ignored`
#[cfg(test)]
mod db_tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use axum::Router;
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tower::ServiceExt;
    use yukipan_config::Config;
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
            config,
        };
        let app = crate::router(state.clone());
        Fixture {
            _tmp: tmp,
            app,
            state,
        }
    }

    async fn login(app: &Router, username: &str, password: &str) -> String {
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"username":"{username}","password":"{password}"}}"#
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

    async fn upload(
        app: &Router,
        cookie: &str,
        dir: &str,
        filename: &str,
        content: &[u8],
    ) -> (StatusCode, Value) {
        let (ct, body) = multipart(filename, content);
        let uri = format!("/api/fs/upload?path={dir}");
        let resp = call(app, "POST", &uri, Some(cookie), Some(&ct), Body::from(body)).await;
        let status = resp.status();
        let v = json_body(resp).await;
        (status, v)
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn private_fs_full_flow() {
        let f = fixture("").await;
        let tag = Uuid::new_v4();
        let u1 = f
            .state
            .store
            .create_user(&format!("t1-{tag}"), "pw1")
            .await
            .unwrap();
        let u2 = f
            .state
            .store
            .create_user(&format!("t2-{tag}"), "pw2")
            .await
            .unwrap();
        let c1 = login(&f.app, &format!("t1-{tag}"), "pw1").await;
        let c2 = login(&f.app, &format!("t2-{tag}"), "pw2").await;

        // 上传: 新内容 deduped=false (内容带 run 唯一后缀: dev 库持久,
        // 历史 run 的残留行不能影响本次 ingest 的判重与落盘)
        let content = format!("hello yukipan {tag}").into_bytes();
        let (st, v) = upload(&f.app, &c1, "", "hello.txt", &content).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["data"]["deduped"], false);
        assert_eq!(v["data"]["path"], "hello.txt");
        assert_eq!(v["data"]["size"], content.len());
        let sha = v["data"]["sha256"].as_str().unwrap().to_string();

        // 列表能看到
        let resp = call(&f.app, "GET", "/api/fs/list?path=", Some(&c1), None, Body::empty()).await;
        let v = json_body(resp).await;
        assert!(v["data"].as_array().unwrap().iter().any(|e| e["name"] == "hello.txt"));

        // 同内容再传: deduped=true, 但账上加一份
        let (st, v) = upload(&f.app, &c1, "", "copy.txt", &content).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["data"]["deduped"], true);

        // 秒传: 已持有指向 → 放行
        let (st, v) = {
            let body = json!({"path": "", "name": "inst.txt", "sha256": sha, "size": content.len()}).to_string();
            let resp = call(&f.app, "POST", "/api/fs/instant", Some(&c1), Some("application/json"), Body::from(body)).await;
            (resp.status(), json_body(resp).await)
        };
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["data"]["deduped"], true);

        // 秒传防猜: 别的用户没指向 → 404; 瞎编的 hash → 404; 大小不符 → 400
        let bogus = "0".repeat(64);
        for (cookie, hash, size, expect) in [
            (c2.as_str(), sha.as_str(), content.len() as u64, StatusCode::NOT_FOUND),
            (c1.as_str(), bogus.as_str(), content.len() as u64, StatusCode::NOT_FOUND),
            (c1.as_str(), sha.as_str(), 999u64, StatusCode::BAD_REQUEST),
        ] {
            let body = json!({"path": "", "name": "x.txt", "sha256": hash, "size": size}).to_string();
            let resp = call(&f.app, "POST", "/api/fs/instant", Some(cookie), Some("application/json"), Body::from(body)).await;
            assert_eq!(resp.status(), expect);
        }

        // download 头部断言
        let resp = call(&f.app, "GET", "/api/fs/download?path=hello.txt", Some(&c1), None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("x-accel-redirect").unwrap(),
            &format!("/protected/{}/hello.txt", u1.id)
        );
        let disp = resp.headers().get("content-disposition").unwrap().to_str().unwrap();
        assert!(disp.starts_with("attachment") && disp.contains("filename*=UTF-8''hello.txt"));

        // preview 头部断言: txt → 纯文本 + nosniff
        let resp = call(&f.app, "GET", "/api/fs/preview?path=hello.txt", Some(&c1), None, Body::empty()).await;
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(resp.headers().get("x-content-type-options").unwrap(), "nosniff");

        // 路径穿越: `..` 与 %2e%2e → 400; 绝对路径折叠成根内 → 404
        for (uri, expect) in [
            ("/api/fs/list?path=../etc", StatusCode::BAD_REQUEST),
            ("/api/fs/list?path=%2e%2e/etc", StatusCode::BAD_REQUEST),
            ("/api/fs/list?path=/etc/passwd", StatusCode::NOT_FOUND),
            ("/api/fs/download?path=..%2fsecret", StatusCode::BAD_REQUEST),
        ] {
            let resp = call(&f.app, "GET", uri, Some(&c1), None, Body::empty()).await;
            assert_eq!(resp.status(), expect, "{uri}");
        }

        // 覆盖拒绝: copy.txt 已存在 → 409
        let (st, _) = upload(&f.app, &c1, "", "copy.txt", &content).await;
        assert_eq!(st, StatusCode::CONFLICT);

        // mkdir + move 文件: 指向路径跟着改
        let resp = call(&f.app, "POST", "/api/fs/mkdir", Some(&c1), Some("application/json"), Body::from(r#"{"path":"docs"}"#)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = call(&f.app, "POST", "/api/fs/move", Some(&c1), Some("application/json"), Body::from(r#"{"from":"hello.txt","to":"docs/renamed.txt"}"#)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = call(&f.app, "GET", "/api/fs/download?path=docs/renamed.txt", Some(&c1), None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = call(&f.app, "GET", "/api/fs/download?path=hello.txt", Some(&c1), None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // move 目录: 其下指向按前缀平移
        let content3 = format!("inner file content {tag}").into_bytes();
        let (st, _) = upload(&f.app, &c1, "docs", "inner.txt", &content3).await;
        assert_eq!(st, StatusCode::OK);
        let resp = call(&f.app, "POST", "/api/fs/move", Some(&c1), Some("application/json"), Body::from(r#"{"from":"docs","to":"docs2"}"#)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = call(&f.app, "GET", "/api/fs/download?path=docs2/inner.txt", Some(&c1), None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = call(&f.app, "GET", "/api/fs/download?path=docs2/renamed.txt", Some(&c1), None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = call(&f.app, "GET", "/api/fs/download?path=docs/inner.txt", Some(&c1), None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 中文文件名: redirect 百分号编码
        let content2 = format!("中文内容 {tag}").into_bytes();
        let (st, _) = upload(&f.app, &c1, "", "中文.txt", &content2).await;
        assert_eq!(st, StatusCode::OK);
        let resp = call(&f.app, "GET", "/api/fs/download?path=%E4%B8%AD%E6%96%87.txt", Some(&c1), None, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("x-accel-redirect").unwrap(),
            &format!("/protected/{}/%E4%B8%AD%E6%96%87.txt", u1.id)
        );

        // 删除 + 账本对账: 当前 refs = docs2/renamed + copy + inst (各 content 长)
        //                  + docs2/inner + 中文.txt
        let l1 = content.len() as u64;
        let l2 = content2.len() as u64;
        let l3 = content3.len() as u64;
        let used = f.state.store.usage_get(u1.id, Space::Private).await.unwrap();
        assert_eq!(used, 3 * l1 + l2 + l3, "当前账本 {used}");

        // 删 copy.txt 与 inst.txt: blob C 还剩 docs2/renamed.txt 一条指向, 不消失
        let resp = call(&f.app, "POST", "/api/fs/delete", Some(&c1), Some("application/json"), Body::from(r#"{"path":"copy.txt","recursive":false}"#)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = call(&f.app, "POST", "/api/fs/delete", Some(&c1), Some("application/json"), Body::from(r#"{"path":"inst.txt","recursive":false}"#)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(f.state.blobs.blob_file_path(&sha).exists());
        assert_eq!(f.state.blobs.blob_ref_count(&sha).await.unwrap(), 1);

        // 递归删目录: docs2 下两条指向 (renamed.txt 与 inner.txt) 一起收尾,
        // renamed 是 blob C 最后一条指向 → 引用归 0, blob 行与盘上文件都消失
        let resp = call(&f.app, "POST", "/api/fs/delete", Some(&c1), Some("application/json"), Body::from(r#"{"path":"docs2","recursive":true}"#)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(f.state.blobs.blob_ref_count(&sha).await.unwrap(), 0);
        assert!(!f.state.blobs.blob_file_path(&sha).exists());

        // 账本最终只剩 中文.txt 一份
        let used = f.state.store.usage_get(u1.id, Space::Private).await.unwrap();
        assert_eq!(used, l2, "收尾账本 {used}");
        // 另一用户账本独立
        assert_eq!(f.state.store.usage_get(u2.id, Space::Private).await.unwrap(), 0);
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn upload_quota_gate() {
        let f = fixture("private_limit = 1024").await;
        let tag = Uuid::new_v4();
        f.state
            .store
            .create_user(&format!("tq-{tag}"), "pw")
            .await
            .unwrap();
        let c = login(&f.app, &format!("tq-{tag}"), "pw").await;

        // 明显超限: Content-Length 预检直接 413
        let big = vec![b'x'; 5000];
        let (st, _) = upload(&f.app, &c, "", "big.bin", &big).await;
        assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);

        // 小文件过闸; 再来一个把账本顶爆 → 413 (内容带 run 唯一后缀, 防历史行干扰)
        let a = tag.to_string().repeat(30).into_bytes();
        let b = format!("b{tag}").repeat(30).into_bytes();
        let (st, v) = upload(&f.app, &c, "", "a.bin", &a[..900]).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let (st, _) = upload(&f.app, &c, "", "b.bin", &b[..900]).await;
        assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
