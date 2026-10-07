//! 访客空间域 (文档第 4、6 章): 访客指向与 TTL 清扫。
//!
//! 访客没有目录树也没有公开整仓列表; 指向行挂在 [`BlobStore`] 上
//! (与 public/guest/ 下的公开 hardlink 同生命周期, 由 API 层/清扫任务组合)。
//! 配额账记在 `charged_to` 用户的 guest 空间: 匿名上传记 owner (最早创建的用户,
//! 单用户应用即管理员), 登录分享记分享者。

use std::net::IpAddr;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{BlobStore, Result, Space, Store, StoreError, sql_safe};
use tracing::warn;

/// 访客指向记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestRef {
    pub id: Uuid,
    pub public_name: String,
    pub orig_name: String,
    pub sha256: String,
    pub size: u64,
    pub source_ip: IpAddr,
    /// 这条指向的 guest 配额记在谁账上。
    pub charged_to: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

type GuestRow = (
    Uuid,
    String,
    String,
    String,
    i64,
    IpAddr,
    Uuid,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn row_to_guest(r: GuestRow) -> GuestRef {
    GuestRef {
        id: r.0,
        public_name: r.1,
        orig_name: r.2,
        sha256: r.3,
        size: r.4 as u64,
        source_ip: r.5,
        charged_to: r.6,
        created_at: r.7,
        expires_at: r.8,
    }
}

const GUEST_SELECT: &str =
    "SELECT id, public_name, orig_name, sha256, size, source_ip, charged_to, created_at, expires_at
     FROM guest_refs";

impl BlobStore {
    /// 访客配额挂账对象: 最早创建的用户 (单用户应用即管理员)。
    /// 库里一个用户都没有时返回 None — 调用方跳过记账 (此时也没有任何账可记)。
    pub async fn owner_user_id(&self) -> Result<Option<Uuid>> {
        let row: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM users ORDER BY created_at LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(id,)| id))
    }

    /// 插一条访客指向。
    pub async fn insert_guest_ref(
        &self,
        public_name: &str,
        orig_name: &str,
        sha256: &str,
        size: u64,
        source_ip: IpAddr,
        charged_to: Uuid,
        expires_at: DateTime<Utc>,
    ) -> Result<GuestRef> {
        let row = sqlx::query_as::<_, GuestRow>(
            "INSERT INTO guest_refs (public_name, orig_name, sha256, size, source_ip, charged_to, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id, public_name, orig_name, sha256, size, source_ip, charged_to, created_at, expires_at",
        )
        .bind(public_name)
        .bind(orig_name)
        .bind(sha256)
        .bind(i64::try_from(size).map_err(|_| StoreError::TooLarge)?)
        .bind(source_ip)
        .bind(charged_to)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(db) = &e
                && db.is_foreign_key_violation()
            {
                return StoreError::BlobNotFound;
            }
            StoreError::Db(e)
        })?;
        Ok(row_to_guest(row))
    }

    /// 登录管理列表 (访客没有公开整仓列表), offset 分页, 新的在前。
    pub async fn list_guest_refs(&self, page: u32, per_page: u32) -> Result<(Vec<GuestRef>, u64)> {
        let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM guest_refs")
            .fetch_one(&self.pool)
            .await?;
        let rows = sqlx::query_as::<_, GuestRow>(sql_safe(format!(
            "{GUEST_SELECT} ORDER BY created_at DESC, id DESC LIMIT $1 OFFSET $2"
        )))
        .bind(i64::from(per_page))
        .bind(i64::from(page.saturating_sub(1)) * i64::from(per_page))
        .fetch_all(&self.pool)
        .await?;
        Ok((rows.into_iter().map(row_to_guest).collect(), total as u64))
    }

    /// 按 id 取访客指向 (没有返回 None)。
    pub async fn get_guest_ref(&self, id: Uuid) -> Result<Option<GuestRef>> {
        let row = sqlx::query_as::<_, GuestRow>(sql_safe(format!("{GUEST_SELECT} WHERE id = $1")))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(row_to_guest))
    }

    /// 按公开名取访客指向 (下载校验/测试用)。
    pub async fn get_guest_ref_by_name(&self, public_name: &str) -> Result<Option<GuestRef>> {
        let row = sqlx::query_as::<_, GuestRow>(sql_safe(format!(
            "{GUEST_SELECT} WHERE public_name = $1"
        )))
        .bind(public_name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_guest))
    }

    /// 删一条访客指向, 返回被删的行 (调用方做盘上/账本/blob 收尾)。
    pub async fn delete_guest_ref(&self, id: Uuid) -> Result<Option<GuestRef>> {
        let row = sqlx::query_as::<_, GuestRow>(sql_safe(format!(
            "{GUEST_SELECT} WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row.map(row_to_guest) else {
            return Ok(None);
        };
        sqlx::query("DELETE FROM guest_refs WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(Some(r))
    }

    /// 全量 (一键清空 / 清扫前的枚举)。
    pub async fn list_all_guest_refs(&self) -> Result<Vec<GuestRef>> {
        let rows = sqlx::query_as::<_, GuestRow>(sql_safe(format!("{GUEST_SELECT} ORDER BY created_at")))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(row_to_guest).collect())
    }

    /// 到点未清的访客指向 (TTL 清扫用, 文档第 4、6 章)。
    pub async fn list_expired_guest_refs(&self, now: DateTime<Utc>) -> Result<Vec<GuestRef>> {
        let rows = sqlx::query_as::<_, GuestRow>(sql_safe(format!(
            "{GUEST_SELECT} WHERE expires_at < $1 ORDER BY expires_at"
        )))
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_guest).collect())
    }
}

/// 删一条访客指向的完整收尾 (delete/clear/TTL 清扫共用):
/// 库行已删 → 删公开 hardlink → 减账 → blob 引用计数归 0 收尾。
/// 收尾失败只记日志, 不中断批量流程。
pub async fn finish_guest_ref_delete(blobs: &BlobStore, store: &Store, r: &GuestRef) {
    let path = blobs
        .data_root()
        .join("public/guest")
        .join(&r.public_name);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        warn!("删访客公开文件失败 ({}): {e}", r.public_name);
    }
    if let Err(e) = store.usage_sub(r.charged_to, Space::Guest, r.size).await {
        warn!("删访客指向后减账失败 (账本可能漂移): {e}");
    }
    if let Err(e) = blobs.delete_blob_if_unreferenced(&r.sha256).await {
        warn!("删访客指向后清理 blob 失败 ({}): {e}", r.sha256);
    }
}

/// TTL 清扫 (文档第 6 章: 每小时跑一次): 过期的访客指向逐条走删除流水线。
/// 返回清掉的条数; 单条失败只记日志继续扫。
pub async fn sweep_expired_guests(blobs: &BlobStore, store: &Store) -> Result<u64> {
    let expired = blobs.list_expired_guest_refs(Utc::now()).await?;
    let mut swept = 0;
    for r in expired {
        // delete 返回 None 说明已被并发的 delete/clear 删掉, 跳过。
        let Some(r) = blobs.delete_guest_ref(r.id).await? else {
            continue;
        };
        finish_guest_ref_delete(blobs, store, &r).await;
        swept += 1;
    }
    Ok(swept)
}
