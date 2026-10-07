//! 访客密钥门 (文档第 4、5 章): 管理员签发临时上传密钥, 持密钥才能匿名上传。
//!
//! 密钥三档 30m/1h/24h; 8 位短码 (字母数字去掉易混淆字符), 验证时大小写
//! 不敏感、允许 `-`/空格分组。文件寿命 = 密钥寿命 (上传时指向的 expires_at
//! 直接取密钥的), 吊销 = 主动级联删指向, 过期由 TTL 清扫自然覆盖。

use chrono::{DateTime, Duration, Utc};
use password_hash::rand_core::{OsRng, RngCore};
use uuid::Uuid;

use crate::{BlobStore, GuestRef, Result, StoreError};

/// 短码字母表: 去掉 0/O/1/I/L 后的 31 个字符 (规格写 32, 实际去掉 5 个字符是 31;
/// 31^8 ≈ 8.5e11 配合唯一冲突重试足够用)。
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// 短码长度。
pub const CODE_LEN: usize = 8;

/// 碰撞重试上限 (理论上几乎不会撞, 兜底防御)。
const CODE_RETRY: usize = 5;

/// 密钥记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestKey {
    pub id: Uuid,
    pub code: String,
    pub note: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

type KeyRow = (Uuid, String, String, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>);

fn row_to_key(r: KeyRow) -> GuestKey {
    GuestKey {
        id: r.0,
        code: r.1,
        note: r.2,
        created_at: r.3,
        expires_at: r.4,
        revoked_at: r.5,
    }
}

const KEY_SELECT: &str = "SELECT id, code, note, created_at, expires_at, revoked_at FROM guest_keys";

/// 短码归一化: 去 `-` 与空白、转大写; 不合法 (长度/字母表不符) 返回 None。
pub fn normalize_code(input: &str) -> Option<String> {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if cleaned.len() == CODE_LEN && cleaned.bytes().all(|b| CODE_ALPHABET.contains(&b)) {
        Some(cleaned)
    } else {
        None
    }
}

/// 生成一个短码 (OsRng)。
fn generate_code() -> String {
    let mut buf = [0u8; CODE_LEN];
    OsRng.fill_bytes(&mut buf);
    buf.iter()
        .map(|b| CODE_ALPHABET[(b % CODE_ALPHABET.len() as u8) as usize] as char)
        .collect()
}

impl BlobStore {
    /// 签发密钥: 随机短码, 撞 UNIQUE 重试。
    pub async fn create_guest_key(&self, ttl: Duration, note: &str) -> Result<GuestKey> {
        for _ in 0..CODE_RETRY {
            let code = generate_code();
            let expires_at = Utc::now() + ttl;
            let row = sqlx::query_as::<_, KeyRow>(
                "INSERT INTO guest_keys (code, note, expires_at) VALUES ($1, $2, $3)
                 RETURNING id, code, note, created_at, expires_at, revoked_at",
            )
            .bind(&code)
            .bind(note)
            .bind(expires_at)
            .fetch_one(&self.pool)
            .await;
            match row {
                Ok(r) => return Ok(row_to_key(r)),
                Err(sqlx::Error::Database(db)) if db.is_unique_violation() => continue,
                Err(e) => return Err(StoreError::Db(e)),
            }
        }
        // 连撞 CODE_RETRY 次, 基本不可能, 兜底报错
        Err(StoreError::Db(sqlx::Error::Protocol(
            "短码生成多次撞车".into(),
        )))
    }

    /// 校验短码: 归一化后存在、未吊销、未过期才返回密钥; 其余一律 None
    /// (不区分「不存在/过期/吊销」, 防探测)。
    pub async fn verify_guest_key(&self, code: &str) -> Result<Option<GuestKey>> {
        let Some(code) = normalize_code(code) else {
            return Ok(None);
        };
        let row = sqlx::query_as::<_, KeyRow>(crate::sql_safe(format!(
            "{KEY_SELECT} WHERE code = $1 AND revoked_at IS NULL AND expires_at > now()"
        )))
        .bind(&code)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_key))
    }

    /// 密钥列表 (管理侧), 创建时间倒序, 带现存指向数。
    pub async fn list_guest_keys(&self) -> Result<Vec<(GuestKey, u64)>> {
        let rows = sqlx::query_as::<
            _,
            (Uuid, String, String, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>, i64),
        >(
            "SELECT guest_keys.id, code, note, guest_keys.created_at, expires_at, revoked_at,
                    (SELECT COUNT(*) FROM guest_refs r WHERE r.key_id = guest_keys.id)
             FROM guest_keys ORDER BY guest_keys.created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    GuestKey {
                        id: r.0,
                        code: r.1,
                        note: r.2,
                        created_at: r.3,
                        expires_at: r.4,
                        revoked_at: r.5,
                    },
                    r.6 as u64,
                )
            })
            .collect())
    }

    /// 吊销密钥: 置 revoked_at, 并删掉该密钥全部指向 (行级)。
    /// 返回被删的指向行 (调用方走 finish_guest_ref_delete 做盘上/账本/blob 收尾)。
    /// 不存在报 [`StoreError::GuestKeyNotFound`], 已吊销报 [`StoreError::GuestKeyRevoked`]。
    pub async fn revoke_guest_key(&self, id: Uuid) -> Result<Vec<GuestRef>> {
        let row = sqlx::query_as::<_, KeyRow>(crate::sql_safe(format!(
            "{KEY_SELECT} WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(key) = row.map(row_to_key) else {
            return Err(StoreError::GuestKeyNotFound);
        };
        if key.revoked_at.is_some() {
            return Err(StoreError::GuestKeyRevoked);
        }
        sqlx::query("UPDATE guest_keys SET revoked_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        // 逐行走标准删除 (行没了才算指向消失), 收尾由调用方复用流水线。
        let refs = self.list_guest_refs_by_key(id).await?;
        let mut deleted = Vec::with_capacity(refs.len());
        for r in refs {
            if let Some(r) = self.delete_guest_ref(r.id).await? {
                deleted.push(r);
            }
        }
        Ok(deleted)
    }

    /// 某密钥的全部指向 (吊销级联用)。
    async fn list_guest_refs_by_key(&self, key_id: Uuid) -> Result<Vec<GuestRef>> {
        let rows = sqlx::query_as::<_, crate::guest::GuestRow>(crate::sql_safe(format!(
            "{} WHERE g.key_id = $1 ORDER BY g.created_at",
            crate::guest::GUEST_SELECT
        )))
        .bind(key_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(crate::guest::row_to_guest).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_normalization() {
        // 大小写不敏感, 允许 `-`/空格分组
        assert_eq!(normalize_code("abcd2345"), Some("ABCD2345".into()));
        assert_eq!(normalize_code("ABCD-2345"), Some("ABCD2345".into()));
        assert_eq!(normalize_code(" abcd 2345 "), Some("ABCD2345".into()));
        assert_eq!(normalize_code("AB-CD 23-45"), Some("ABCD2345".into()));
        // 易混淆字符不在字母表
        assert_eq!(normalize_code("ABCD234O"), None);
        assert_eq!(normalize_code("ABCD2340"), None);
        assert_eq!(normalize_code("ABCD234I"), None);
        assert_eq!(normalize_code("ABCD234L"), None);
        // 长度不对
        assert_eq!(normalize_code("ABCD234"), None);
        assert_eq!(normalize_code("ABCD23456"), None);
        assert_eq!(normalize_code(""), None);
    }

    #[test]
    fn generated_codes_are_valid() {
        for _ in 0..100 {
            let code = generate_code();
            assert_eq!(code.len(), CODE_LEN);
            assert_eq!(normalize_code(&code).as_deref(), Some(code.as_str()));
        }
    }
}
