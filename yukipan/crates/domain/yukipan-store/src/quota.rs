//! 配额两道闸 (文档第 6 章): 引用配额账本 + 磁盘余量红线。
//!
//! 账本按引用计: 同一内容挂多个路径账上加多份, 盘上仍是一份。
//! 收新内容要同时过两道: 对应空间配额没超, 且盘上可用 > reserve。

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use uuid::Uuid;

use crate::{BlobStore, Result, Store, StoreError};

/// 配额空间, 取值与 usage 表的 CHECK 约束一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// 私有存储。
    Private,
    /// 图床。
    Images,
    /// 访客空间。
    Guest,
}

impl Space {
    /// 账本里的空间标识。
    pub fn as_str(&self) -> &'static str {
        match self {
            Space::Private => "private",
            Space::Images => "images",
            Space::Guest => "guest",
        }
    }
}

impl Store {
    /// 查某用户某空间已用字节 (无账本行视为 0)。
    pub async fn usage_get(&self, user_id: Uuid, space: Space) -> Result<u64> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT used_bytes FROM usage WHERE user_id = $1 AND space = $2",
        )
        .bind(user_id)
        .bind(space.as_str())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map_or(0, |(n,)| n as u64))
    }

    /// 第一道闸的只读预检: 已用 + 拟收 > limit 则拒绝 (文档第 6 章)。
    /// limit 来自 Config.quota 对应字段, 由调用方按空间传入。
    ///
    /// 这只是提前拒收的提示性检查; 真正防并发超收靠 [`Store::usage_add`]
    /// 的原子条件更新, 收完内容务必走它记账。
    pub async fn check_quota(
        &self,
        user_id: Uuid,
        space: Space,
        bytes: u64,
        limit: u64,
    ) -> Result<()> {
        let used = self.usage_get(user_id, space).await?;
        match used.checked_add(bytes) {
            Some(total) if total <= limit => Ok(()),
            _ => Err(StoreError::QuotaExceeded),
        }
    }

    /// 加账 (收编新引用): 原子「条件更新 + UPSERT」, 超 limit 一行不动返回
    /// [`StoreError::QuotaExceeded`], 并发加账不会双双越过限额。返回新已用。
    pub async fn usage_add(
        &self,
        user_id: Uuid,
        space: Space,
        delta: u64,
        limit: u64,
    ) -> Result<u64> {
        let delta = i64::try_from(delta).map_err(|_| StoreError::TooLarge)?;
        let limit = i64::try_from(limit).map_err(|_| StoreError::TooLarge)?;
        // 单行语句完成判限与加账, 靠 ON CONFLICT 的 WHERE 挡超额;
        // 并发时行锁串行化, 后到者在新的 used_bytes 上重估。
        let row: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO usage (user_id, space, used_bytes) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, space) DO UPDATE
             SET used_bytes = usage.used_bytes + EXCLUDED.used_bytes
             WHERE usage.used_bytes + EXCLUDED.used_bytes <= $4
             RETURNING used_bytes",
        )
        .bind(user_id)
        .bind(space.as_str())
        .bind(delta)
        .bind(limit)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(n,)| n as u64).ok_or(StoreError::QuotaExceeded)
    }

    /// 减账 (撤引用): 余额不足一行不动返回 [`StoreError::UsageUnderflow`],
    /// 账本绝不减成负数。返回新已用。
    pub async fn usage_sub(&self, user_id: Uuid, space: Space, delta: u64) -> Result<u64> {
        let delta = i64::try_from(delta).map_err(|_| StoreError::TooLarge)?;
        let row: Option<(i64,)> = sqlx::query_as(
            "UPDATE usage SET used_bytes = used_bytes - $3
             WHERE user_id = $1 AND space = $2 AND used_bytes >= $3
             RETURNING used_bytes",
        )
        .bind(user_id)
        .bind(space.as_str())
        .bind(delta)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(n,)| n as u64).ok_or(StoreError::UsageUnderflow)
    }
}

impl BlobStore {
    /// 第二道闸: 数据根所在文件系统的可用空间 (对运行用户) 必须 > reserve
    /// (文档第 6 章; tmp/库/日志不进账本, 靠这道兜底)。返回可用字节数。
    pub fn check_disk_reserve(&self, reserve: u64) -> Result<u64> {
        let free = disk_available_bytes(self.data_root())?;
        if free > reserve {
            Ok(free)
        } else {
            Err(StoreError::DiskReserve { free, reserve })
        }
    }
}

/// statvfs 可用字节 (f_bavail × f_frsize, 即非 root 用户视角)。
fn disk_available_bytes(path: &Path) -> std::io::Result<u64> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "路径含 NUL"))?;
    // statvfs 无失败内存语义, zeroed 是合法初值。
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(u64::from(stat.f_bavail) * stat.f_frsize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;
    use tempfile::TempDir;

    #[test]
    fn disk_available_bytes_reports_tmpdir() {
        let tmp = TempDir::new().unwrap();
        assert!(disk_available_bytes(tmp.path()).unwrap() > 0);
        assert!(disk_available_bytes(&tmp.path().join("missing")).is_err());
    }

    #[tokio::test]
    async fn disk_reserve_gate() {
        let tmp = TempDir::new().unwrap();
        // 磁盘闸不碰库, 用懒连接池即可。
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/none")
            .unwrap();
        let blobs = BlobStore::new(pool, tmp.path());
        let free = blobs.check_disk_reserve(0).unwrap();
        assert!(free > 0);
        assert!(matches!(
            blobs.check_disk_reserve(u64::MAX),
            Err(StoreError::DiskReserve { .. })
        ));
    }

    async fn store() -> Store {
        let url = std::env::var("YUKIPAN_TEST_DB_URL").expect("缺少 YUKIPAN_TEST_DB_URL");
        let pool = yukipan_db::connect(&url).await.unwrap();
        yukipan_db::migrate(&pool).await.unwrap();
        Store::new(pool)
    }

    /// `YUKIPAN_TEST_DB_URL=postgres://... cargo test -p yukipan-store -- --ignored`
    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn usage_add_sub_boundaries() {
        let store = store().await;
        let user = store
            .create_user(&format!("test-{}", Uuid::new_v4()), "pw")
            .await
            .unwrap();

        assert_eq!(store.usage_get(user.id, Space::Private).await.unwrap(), 0);
        assert_eq!(
            store.usage_add(user.id, Space::Private, 100, 1000).await.unwrap(),
            100
        );
        // 预检: 900 过, 901 拒
        store.check_quota(user.id, Space::Private, 900, 1000).await.unwrap();
        assert!(matches!(
            store.check_quota(user.id, Space::Private, 901, 1000).await,
            Err(StoreError::QuotaExceeded)
        ));
        // 原子加账同样拒 901, 收 900 到满
        assert!(matches!(
            store.usage_add(user.id, Space::Private, 901, 1000).await,
            Err(StoreError::QuotaExceeded)
        ));
        assert_eq!(
            store.usage_add(user.id, Space::Private, 900, 1000).await.unwrap(),
            1000
        );
        // 减账: 500 过, 再减 501 拒 (不能减成负数)
        assert_eq!(
            store.usage_sub(user.id, Space::Private, 500).await.unwrap(),
            500
        );
        assert!(matches!(
            store.usage_sub(user.id, Space::Private, 501).await,
            Err(StoreError::UsageUnderflow)
        ));
        // 空间之间账本独立
        assert_eq!(store.usage_get(user.id, Space::Images).await.unwrap(), 0);
    }
}
