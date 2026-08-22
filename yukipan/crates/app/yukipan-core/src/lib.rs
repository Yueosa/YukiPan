//! YukiPan 装配层: 配置、库连接、域名服务、HTTP 监听。

use std::path::PathBuf;

use thiserror::Error;
use yukipan_config::{Config, ConfigError};
use yukipan_db::DbError;

/// 装配/运行期错误。
#[derive(Debug, Error)]
pub enum CoreError {
    /// 配置问题。
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// 数据库问题。
    #[error(transparent)]
    Db(#[from] DbError),
    /// 监听/服务 IO 问题。
    #[error("HTTP 服务失败: {0}")]
    Io(#[from] std::io::Error),
}

/// 装配并运行, 直到收到 SIGINT/SIGTERM 后优雅退出。
pub async fn run() -> Result<(), CoreError> {
    let config = Config::load(config_path())?;
    let pool = yukipan_db::connect(&config.database_url).await?;
    yukipan_db::migrate(&pool).await?;
    let app = yukipan_api::router();
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// 配置文件位置: `YUKIPAN_CONFIG` 优先, 否则 `~/.YukiPan/config.toml`。
fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("YUKIPAN_CONFIG") {
        return PathBuf::from(p);
    }
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".YukiPan/config.toml")
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
