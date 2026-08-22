//! YukiPan PostgreSQL 基建: 连接池与迁移。
//!
//! 机器内存有限, 池刻意开小; 迁移文件放在本 crate 的 `migrations/` 下,
//! 各业务表随所属切片加入。

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use thiserror::Error;

/// 数据库基建错误。
#[derive(Debug, Error)]
pub enum DbError {
    /// 连不上 PostgreSQL。
    #[error("连接 PostgreSQL 失败: {0}")]
    Connect(#[source] sqlx::Error),
    /// 迁移执行失败。
    #[error("执行迁移失败: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// 建连接池。
pub async fn connect(database_url: &str) -> Result<PgPool, DbError> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .map_err(DbError::Connect)
}

/// 执行尚未应用的迁移。
pub async fn migrate(pool: &PgPool) -> Result<(), DbError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 需要真实库:
    /// `YUKIPAN_TEST_DB_URL=postgres://... cargo test -p yukipan-db -- --ignored`
    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn connect_and_migrate() {
        let url = std::env::var("YUKIPAN_TEST_DB_URL").expect("缺少 YUKIPAN_TEST_DB_URL");
        let pool = connect(&url).await.unwrap();
        migrate(&pool).await.unwrap();
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(n >= 1);
    }
}
