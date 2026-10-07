//! YukiPan 装配层: 配置、库连接、域名服务、HTTP 监听、管理子命令。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use thiserror::Error;
use yukipan_api::AppState;
use yukipan_config::{Config, ConfigError};
use yukipan_db::DbError;
use yukipan_limit::Limiter;
use yukipan_store::{BlobStore, Store, StoreError};
use tracing::{error, info};

/// 装配/运行期错误。
#[derive(Debug, Error)]
pub enum CoreError {
    /// 配置问题。
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// 数据库问题。
    #[error(transparent)]
    Db(#[from] DbError),
    /// 域操作问题。
    #[error(transparent)]
    Store(#[from] StoreError),
    /// 监听/服务 IO 问题。
    #[error("HTTP 服务失败: {0}")]
    Io(#[from] std::io::Error),
    /// user add 两次密码输入不一致。
    #[error("两次输入的密码不一致")]
    PasswordMismatch,
    /// user add 收到空密码。
    #[error("密码不能为空")]
    EmptyPassword,
}

/// 装配并运行, 直到收到 SIGINT/SIGTERM 后优雅退出。
pub async fn run() -> Result<(), CoreError> {
    init_tracing();
    let config = Config::load(config_path())?;
    info!(data_root = %config.data_root.display(), "配置已加载");
    ensure_data_root_layout(&config.data_root)?;
    let pool = yukipan_db::connect(&config.database_url).await?;
    yukipan_db::migrate(&pool).await?;
    info!("数据库迁移完成");
    let state = AppState {
        store: Store::new(pool.clone()),
        blobs: BlobStore::new(pool, &config.data_root),
        // Redis 连不上时 Limiter 内部降级为放行 (防风暴不做单点故障), 详见 yukipan-limit。
        limiter: Limiter::connect(&config.redis_url).await,
        config: config.clone(),
    };
    spawn_guest_sweeper(state.clone());
    let app = yukipan_api::router(state);
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    info!(listen = %config.listen, "HTTP 服务启动");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// TTL 清扫 (文档第 4、6 章): 启动先立即跑一遍 (停机期间过期的也能清掉),
/// 之后每小时一次。单轮失败只记日志, 不中断后续轮次。
fn spawn_guest_sweeper(state: AppState) {
    info!("访客 TTL 清扫任务已启动 (每小时, 启动即扫一轮)");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        // interval 第一次 tick 立即触发, 正好当启动即扫用。
        loop {
            interval.tick().await;
            match yukipan_store::sweep_expired_guests(&state.blobs, &state.store).await {
                Ok(n) if n > 0 => info!("TTL 清扫: 清掉 {n} 条过期访客指向"),
                Ok(_) => {}
                Err(e) => error!("TTL 清扫失败 (下轮重试): {e}"),
            }
        }
    });
}

/// 数据根子目录布局兜底 (文档第 6 章)。生产由 ops/bootstrap-host.sh 建,
/// 这里 create_dir_all 幂等补建, 让开发/手搓环境启动不踩缺目录的坑。
fn ensure_data_root_layout(data_root: &Path) -> Result<(), CoreError> {
    for sub in ["blobs", "private", "public/images", "public/guest", "public/thumbs", "tmp"] {
        std::fs::create_dir_all(data_root.join(sub))?;
    }
    Ok(())
}

/// `yukipan thumbs rebuild`: 遍历图床指向, 缺缩略图的补生成 (存量图搬迁/上线前跑一次)。
pub async fn thumbs_rebuild() -> Result<(), CoreError> {
    let config = Config::load(config_path())?;
    let pool = yukipan_db::connect(&config.database_url).await?;
    let store = Store::new(pool.clone());
    let blobs = BlobStore::new(pool, &config.data_root);
    let refs = store.list_all_image_refs().await?;
    let total = refs.len();
    let (mut generated, mut skipped, mut failed) = (0u64, 0u64, 0u64);
    for (i, (public_name, sha256)) in refs.into_iter().enumerate() {
        if blobs.thumb_url(&public_name).is_some() {
            skipped += 1;
            continue;
        }
        let ok = blobs
            .generate_thumb(&blobs.blob_file_path(&sha256), &public_name)
            .await;
        if ok {
            generated += 1;
        } else {
            failed += 1;
        }
        println!("[{}/{total}] {public_name}: {}", i + 1, if ok { "已生成" } else { "失败 (跳过)" });
    }
    println!("缩略图重建完成: 共 {total}, 新生成 {generated}, 已存在跳过 {skipped}, 失败 {failed}");
    Ok(())
}

/// `yukipan user add <用户名>`: 交互式输两遍密码, 建用户 (bootstrap 也走这里)。
pub async fn user_add(username: &str) -> Result<(), CoreError> {
    let config = Config::load(config_path())?;
    let pool = yukipan_db::connect(&config.database_url).await?;
    let store = Store::new(pool);
    let password = rpassword::prompt_password("密码: ")?;
    if password.is_empty() {
        return Err(CoreError::EmptyPassword);
    }
    let confirm = rpassword::prompt_password("确认密码: ")?;
    if password != confirm {
        return Err(CoreError::PasswordMismatch);
    }
    store.create_user(username, &password).await?;
    println!("用户 {username} 已创建");
    Ok(())
}

/// tracing 初始化: RUST_LOG 环境变量控制级别 (缺省 info), 紧凑单行无颜色
/// (生产走 journal, 颜色与控制字符都是噪音)。
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .compact()
        .init();
}

/// 配置文件位置: `YUKIPAN_CONFIG` 优先, 否则 `/etc/yukipan/config.toml`。
fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("YUKIPAN_CONFIG") {
        return PathBuf::from(p);
    }
    PathBuf::from("/etc/yukipan/config.toml")
}

/// SIGINT 或 SIGTERM 任一到达即返回。
async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("注册 SIGTERM 处理器失败");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
