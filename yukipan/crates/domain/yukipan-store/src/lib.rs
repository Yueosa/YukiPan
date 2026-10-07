//! YukiPan 核心域: blob 落盘、指向与去重、配额、会话与登录。
//!
//! 当前实现: 用户 (argon2id 哈希) 与数据库会话。

use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Duration, Utc};
use password_hash::rand_core::OsRng;
use password_hash::{Error as PwHashError, PasswordHash, SaltString};
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

/// 面向 HTTP/CLI 层的存储入口, 包着连接池。
#[derive(Debug, Clone)]
pub struct Store {
    pool: PgPool,
}

/// 用户视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: Uuid,
    pub username: String,
}

/// 域操作错误。
#[derive(Debug, Error)]
pub enum StoreError {
    /// 底层数据库错误。
    #[error("数据库错误: {0}")]
    Db(#[from] sqlx::Error),
    /// 密码哈希计算失败。
    #[error("密码哈希错误: {0}")]
    Hash(#[from] PwHashError),
    /// 建用户时用户名撞车。
    #[error("用户名已存在")]
    UsernameTaken,
}

type Result<T> = std::result::Result<T, StoreError>;

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// 建用户, 密码以 argon2id 哈希落库。
    pub async fn create_user(&self, username: &str, password: &str) -> Result<User> {
        let hash = hash_password(password).await?;
        let (id, username) = sqlx::query_as::<_, (Uuid, String)>(
            "INSERT INTO users (username, password_hash) VALUES ($1, $2) RETURNING id, username",
        )
        .bind(username)
        .bind(hash)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(db) = &e
                && db.is_unique_violation()
            {
                return StoreError::UsernameTaken;
            }
            StoreError::Db(e)
        })?;
        Ok(User { id, username })
    }

    /// 校验登录。成功返回用户; 失败一律 `Ok(None)`, 不区分用户名错还是密码错。
    pub async fn verify_login(&self, username: &str, password: &str) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, username, password_hash FROM users WHERE username = $1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, username, hash)) = row else {
            // 用户不存在也走一遍同等成本的哈希, 抹掉时间侧信道。
            let _ = hash_password(password).await;
            return Ok(None);
        };
        if verify_password(&hash, password).await? {
            Ok(Some(User { id, username }))
        } else {
            Ok(None)
        }
    }

    /// 开会话, 返回会话 id (即 cookie 里的令牌)。
    pub async fn create_session(&self, user_id: Uuid, ttl: Duration) -> Result<Uuid> {
        let expires_at = Utc::now() + ttl;
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO sessions (user_id, expires_at) VALUES ($1, $2) RETURNING id",
        )
        .bind(user_id)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    /// 按令牌取会话用户; 不存在或已过期返回 `Ok(None)`, 过期行顺带删除。
    pub async fn resolve_session(&self, token: Uuid) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, (Uuid, String, DateTime<Utc>)>(
            "SELECT u.id, u.username, s.expires_at
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.id = $1",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, username, expires_at)) = row else {
            return Ok(None);
        };
        if expires_at <= Utc::now() {
            self.delete_session(token).await?;
            return Ok(None);
        }
        Ok(Some(User { id, username }))
    }

    /// 删会话 (登出)。
    pub async fn delete_session(&self, token: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(token)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// argon2 哈希是阻塞 CPU 的活, 丢进阻塞线程池, 不占 reactor。
async fn hash_password(password: &str) -> Result<String> {
    let password = password.to_owned();
    let hashed =
        tokio::task::spawn_blocking(move || -> std::result::Result<String, PwHashError> {
            let salt = SaltString::generate(&mut OsRng);
            Ok(Argon2::default()
                .hash_password(password.as_bytes(), &salt)?
                .to_string())
        })
        .await
        .expect("哈希线程 panic")?;
    Ok(hashed)
}

async fn verify_password(hash: &str, password: &str) -> Result<bool> {
    let hash = hash.to_owned();
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || -> std::result::Result<bool, PwHashError> {
        let parsed = PasswordHash::new(&hash)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .expect("哈希线程 panic")
    .map_err(StoreError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> Store {
        let url = std::env::var("YUKIPAN_TEST_DB_URL").expect("缺少 YUKIPAN_TEST_DB_URL");
        let pool = yukipan_db::connect(&url).await.unwrap();
        yukipan_db::migrate(&pool).await.unwrap();
        Store::new(pool)
    }

    /// `YUKIPAN_TEST_DB_URL=postgres://... cargo test -p yukipan-store -- --ignored`
    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn user_and_session_lifecycle() {
        let store = store().await;
        let username = format!("test-{}", Uuid::new_v4());
        let user = store.create_user(&username, "s3cret").await.unwrap();
        assert_eq!(user.username, username);

        // 重名拒绝
        assert!(matches!(
            store.create_user(&username, "x").await,
            Err(StoreError::UsernameTaken)
        ));

        // 登录: 密码对 / 密码错 / 用户不存在
        assert!(
            store
                .verify_login(&username, "s3cret")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .verify_login(&username, "wrong")
                .await
                .unwrap()
                .is_none()
        );
        assert!(store.verify_login("nobody", "x").await.unwrap().is_none());

        // 会话: 建 / 取 / 删
        let token = store
            .create_session(user.id, Duration::hours(1))
            .await
            .unwrap();
        let got = store.resolve_session(token).await.unwrap().unwrap();
        assert_eq!(got.id, user.id);
        store.delete_session(token).await.unwrap();
        assert!(store.resolve_session(token).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn expired_session_is_rejected_and_swept() {
        let store = store().await;
        let username = format!("test-{}", Uuid::new_v4());
        let user = store.create_user(&username, "pw").await.unwrap();
        let token = store
            .create_session(user.id, Duration::seconds(-1))
            .await
            .unwrap();
        assert!(store.resolve_session(token).await.unwrap().is_none());
        // 过期行已被顺带删掉
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions WHERE id = $1")
            .bind(token)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
