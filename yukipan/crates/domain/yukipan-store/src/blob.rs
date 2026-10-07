//! blob 内容寻址存储 (文档第 6 章): 流式哈希、去重收编、私有区指向、引用计数删除。
//!
//! 盘上布局 `blobs/{aa}/{sha256}` (aa 为 hash 前两位), 唯一真源是磁盘,
//! `blobs` 表是盘上内容的镜像索引, `private_refs` 记录私有区路径到 blob 的指向。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::task::spawn_blocking;
use uuid::Uuid;
use yukipan_fs::LogicalPath;

use crate::{Result, StoreError};
use tracing::warn;

/// 收编 (ingest) 一个 blob 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestOutcome {
    /// 内容算出的 SHA-256 (64 位小写 hex)。
    pub sha256: String,
    /// 字节大小。
    pub size: u64,
    /// true = 库里/盘上已有同内容, 临时文件被丢弃, 未写新字节 (秒传省盘)。
    pub deduped: bool,
}

/// 私有区指向记录 (list_private_refs_under 的行)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateRef {
    /// 逻辑路径。
    pub path: String,
    /// 指向的 blob hash。
    pub sha256: String,
    /// blob 字节数 (Join blobs 得来, 对账用)。
    pub size: u64,
}

/// blob 存储入口: 连接池 + 数据根 (blobs/ 的父目录)。
///
/// 与 [`crate::Store`] 并列: Store 管纯库的域 (用户/会话/配额账本),
/// BlobStore 管要同时碰文件系统的域 (blob 与指向)。
#[derive(Debug, Clone)]
pub struct BlobStore {
    pub(crate) pool: PgPool,
    data_root: PathBuf,
}

impl BlobStore {
    /// `data_root` 是数据根 (blobs/、tmp/ 的父目录), 不需要预先存在 blobs/ 子目录。
    pub fn new(pool: PgPool, data_root: impl Into<PathBuf>) -> Self {
        Self {
            pool,
            data_root: data_root.into(),
        }
    }

    /// 数据根路径。
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// 盘上 blob 路径: `blobs/{aa}/{sha256}`。仅用于在私有区/公开区建 hardlink
    /// 与内部清理, 不得回给前端。
    pub fn blob_file_path(&self, sha256: &str) -> PathBuf {
        self.data_root
            .join("blobs")
            .join(&sha256[..2])
            .join(sha256)
    }

    /// 查 blob 大小; 库里没有该 hash 返回 None (秒传/下载前的存在性检查)。
    pub async fn blob_size(&self, sha256: &str) -> Result<Option<u64>> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT size FROM blobs WHERE sha256 = $1")
            .bind(sha256)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(n,)| n as u64))
    }

    /// 用户是否已持有至少一条指向该 hash 的私有区指向 (文档第 6 章秒传放行判据:
    /// 只知道 hash、没有任何指向不能秒传, 防猜到私有文件 hash 挂进公开区)。
    pub async fn user_has_private_ref(&self, user_id: Uuid, sha256: &str) -> Result<bool> {
        let (yes,): (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM private_refs WHERE user_id = $1 AND sha256 = $2)",
        )
        .bind(user_id)
        .bind(sha256)
        .fetch_one(&self.pool)
        .await?;
        Ok(yes)
    }

    /// 秒传放行判据的宽口径 (图床用, 文档第 6 章): 私有区或图床任一指向都算
    /// 「已持有」— 把私有文件秒传到图床等于主动公开, 允许。
    pub async fn user_has_any_ref(&self, user_id: Uuid, sha256: &str) -> Result<bool> {
        let (yes,): (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM private_refs WHERE user_id = $1 AND sha256 = $2)
                 OR EXISTS(SELECT 1 FROM image_refs i JOIN albums a ON a.id = i.album_id
                           WHERE a.user_id = $1 AND i.sha256 = $2)",
        )
        .bind(user_id)
        .bind(sha256)
        .fetch_one(&self.pool)
        .await?;
        Ok(yes)
    }

    /// 取某逻辑路径的私有区指向 (hash + 大小), 没有返回 None (分享到图床/访客用)。
    pub async fn get_private_ref(
        &self,
        user_id: Uuid,
        path: &LogicalPath,
    ) -> Result<Option<(String, u64)>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT r.sha256, b.size FROM private_refs r JOIN blobs b ON b.sha256 = r.sha256
             WHERE r.user_id = $1 AND r.path = $2",
        )
        .bind(user_id)
        .bind(path.as_str())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(sha, size)| (sha, size as u64)))
    }

    /// 移动/改名后同步指向路径 (文档第 6 章: 文件落盘写指向, 路径是指向的一部分)。
    /// `from` 是文件时只改那一行; 是目录时其下所有指向按前缀平移。
    /// 用 starts_with 而不是 LIKE: 路径里 `%`/`_` 是合法文件名字符, LIKE 会误配。
    /// 返回受影响行数。
    pub async fn rename_private_refs(
        &self,
        user_id: Uuid,
        from: &LogicalPath,
        to: &LogicalPath,
    ) -> Result<u64> {
        let n = sqlx::query(
            "UPDATE private_refs SET path = $3 || substring(path from char_length($2) + 1)
             WHERE user_id = $1 AND (path = $2 OR starts_with(path, $2 || '/'))",
        )
        .bind(user_id)
        .bind(from.as_str())
        .bind(to.as_str())
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n)
    }

    /// 列某路径 (文件或目录前缀) 下的所有私有区指向, 带 blob 大小 (删除/对账用)。
    /// 与 [`BlobStore::rename_private_refs`] 同一套前缀语义。
    pub async fn list_private_refs_under(
        &self,
        user_id: Uuid,
        prefix: &LogicalPath,
    ) -> Result<Vec<PrivateRef>> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT r.path, r.sha256, b.size
             FROM private_refs r JOIN blobs b ON b.sha256 = r.sha256
             WHERE r.user_id = $1 AND (r.path = $2 OR starts_with(r.path, $2 || '/'))
             ORDER BY r.path",
        )
        .bind(user_id)
        .bind(prefix.as_str())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(path, sha256, size)| PrivateRef {
                path,
                sha256,
                size: size as u64,
            })
            .collect())
    }

    /// 删某路径 (文件或目录前缀) 下的所有私有区指向, 返回行数。
    /// 调用方负责随后对每个受影响 hash 跑 [`BlobStore::delete_blob_if_unreferenced`]
    /// 并减用量账。
    pub async fn delete_private_refs_under(
        &self,
        user_id: Uuid,
        prefix: &LogicalPath,
    ) -> Result<u64> {
        let n = sqlx::query(
            "DELETE FROM private_refs
             WHERE user_id = $1 AND (path = $2 OR starts_with(path, $2 || '/'))",
        )
        .bind(user_id)
        .bind(prefix.as_str())
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n)
    }

    /// 把 tmp 文件收编成 blob: 流式算 SHA-256 (分块读, 不整读内存), 可选校验声明值,
    /// 去重则丢 tmp, 否则原子链到 `blobs/{aa}/{sha256}` 并插库行。
    ///
    /// 任何失败路径都会尝试删掉 tmp 文件, 不留上传残渣。
    ///
    /// 并发安全: 两个同内容上传同时收编时, 盘上靠 `hard_link` 的 AlreadyExists
    /// 分出胜负 (输家 deduped), 库里靠 `INSERT ... ON CONFLICT DO NOTHING` 兜底;
    /// 崩溃留下的「有文件无行」孤儿会被下一次同内容收编自愈 (文件已在 → deduped,
    /// INSERT 补上行)。
    pub async fn ingest_blob(
        &self,
        tmp_file: impl AsRef<Path>,
        expected_sha256: Option<&str>,
    ) -> Result<IngestOutcome> {
        let tmp = tmp_file.as_ref().to_path_buf();

        // 声明值先过格式闸: 不合法等同不符, 免得拿脏字符串去碰盘上路径。
        if let Some(expected) = expected_sha256 {
            if !is_sha256_hex(expected) {
                let _ = std::fs::remove_file(&tmp);
                return Err(StoreError::HashMismatch);
            }
        }

        let tmp_for_hash = tmp.clone();
        let hashed = spawn_blocking(move || hash_file(&tmp_for_hash))
            .await
            .expect("哈希线程 panic");
        let (sha256, size) = match hashed {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(StoreError::Io(e));
            }
        };

        if let Some(expected) = expected_sha256 {
            if expected != sha256 {
                let _ = std::fs::remove_file(&tmp);
                return Err(StoreError::HashMismatch);
            }
        }

        // 库行优先判重: 已有行说明内容已收编, 直接丢 tmp。
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT size FROM blobs WHERE sha256 = $1")
                .bind(&sha256)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((size,)) = existing {
            let _ = std::fs::remove_file(&tmp);
            return Ok(IngestOutcome {
                sha256,
                size: size as u64,
                deduped: true,
            });
        }

        let blobs_dir = self.data_root.join("blobs");
        let sha_for_place = sha256.clone();
        let tmp_for_place = tmp.clone();
        let placed = spawn_blocking(move || place_blob(&blobs_dir, &tmp_for_place, &sha_for_place))
            .await
            .expect("落盘线程 panic");
        let placed = match placed {
            Ok(p) => p,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(StoreError::Io(e));
            }
        };
        // hard_link 后 tmp 与目标指向同一 inode, 收编语义是「移走」, 删掉 tmp 名。
        let _ = std::fs::remove_file(&tmp);

        sqlx::query("INSERT INTO blobs (sha256, size) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(&sha256)
            .bind(i64::try_from(size).map_err(|_| StoreError::TooLarge)?)
            .execute(&self.pool)
            .await?;

        Ok(IngestOutcome {
            sha256,
            size,
            deduped: !placed,
        })
    }

    /// 写一条私有区指向 (upsert)。`path` 已是规范化逻辑路径。
    ///
    /// 同路径已有指向时覆盖 — 旧 blob 的引用计数随之减一, 调用方 (API 层)
    /// 负责事后对旧 hash 跑 [`BlobStore::delete_blob_if_unreferenced`]。
    pub async fn add_private_ref(
        &self,
        user_id: Uuid,
        path: &LogicalPath,
        sha256: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO private_refs (user_id, path, sha256) VALUES ($1, $2, $3)
             ON CONFLICT (user_id, path) DO UPDATE SET sha256 = EXCLUDED.sha256, created_at = now()",
        )
        .bind(user_id)
        .bind(path.as_str())
        .bind(sha256)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(db) = &e
                && db.is_foreign_key_violation()
            {
                return StoreError::BlobNotFound;
            }
            StoreError::Db(e)
        })?;
        Ok(())
    }

    /// 删一条私有区指向, 返回它指着的 hash (没有该指向返回 None)。
    /// 返回的 hash 应交给 [`BlobStore::delete_blob_if_unreferenced`] 收尾。
    pub async fn remove_private_ref(
        &self,
        user_id: Uuid,
        path: &LogicalPath,
    ) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("DELETE FROM private_refs WHERE user_id = $1 AND path = $2 RETURNING sha256")
                .bind(user_id)
                .bind(path.as_str())
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(sha,)| sha))
    }

    /// 引用计数: 还指着这个 hash 的指向行数 = private_refs + image_refs + guest_refs
    /// 之和 (文档第 6 章: 三个空间都不要了, 磁盘才真正丢掉)。三表已齐。
    pub async fn blob_ref_count(&self, sha256: &str) -> Result<u64> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM private_refs WHERE sha256 = $1)
                  + (SELECT COUNT(*) FROM image_refs WHERE sha256 = $1)
                  + (SELECT COUNT(*) FROM guest_refs WHERE sha256 = $1)",
        )
        .bind(sha256)
        .fetch_one(&self.pool)
        .await?;
        Ok(n as u64)
    }

    /// 引用计数归 0 才真正删 blob (文档第 6 章): 删库行 + 删盘上文件。
    /// 返回是否发生了删除 (false = 不存在或仍被引用)。
    ///
    /// 顺序是「先删行, 再删文件」, 理由: 若反过来先删文件, 崩溃在两者之间会留下
    /// 「有行无文件」的孤儿行, 下载路径会 500; 而先删行后删文件失败只留孤儿文件,
    /// 不挡任何读, 且同内容下次收编会因盘上已存在直接复用 (自愈)。文件删除失败
    /// 仅告警, 不当错误抛出。
    ///
    /// 竞态: 并发的新指向 (任一指向表) 会对 blobs 行加 FOR KEY SHARE 锁, 与本 DELETE
    /// 互斥 — 要么它先提交 (NOT EXISTS 重估后本删除放弃), 要么本删除先提交
    /// (它的 INSERT 撞外键)。不会出现「删了文件还有指向」。
    pub async fn delete_blob_if_unreferenced(&self, sha256: &str) -> Result<bool> {
        let done = sqlx::query(
            "DELETE FROM blobs b
             WHERE b.sha256 = $1
               AND NOT EXISTS (SELECT 1 FROM private_refs r WHERE r.sha256 = b.sha256)
               AND NOT EXISTS (SELECT 1 FROM image_refs i WHERE i.sha256 = b.sha256)
               AND NOT EXISTS (SELECT 1 FROM guest_refs g WHERE g.sha256 = b.sha256)",
        )
        .bind(sha256)
        .execute(&self.pool)
        .await?
        .rows_affected()
            > 0;
        if !done {
            return Ok(false);
        }
        let path = self.blob_file_path(sha256);
        if let Err(e) = spawn_blocking(move || std::fs::remove_file(path))
            .await
            .expect("删文件线程 panic")
            && e.kind() != ErrorKind::NotFound
        {
            warn!("删除 blob 文件失败 ({sha256}): {e} (留孤儿文件, 不影响一致性)");
        }
        Ok(true)
    }
}

/// 64 位小写 hex 校验。
fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// 流式算 SHA-256, 1 MiB 分块, 返回 (hex, 字节数)。阻塞活, 调用方负责 spawn_blocking。
fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((to_hex(&hasher.finalize()), size))
}

/// 转小写 hex (sha2 0.11 的输出数组不再自带 LowerHex)。
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// 把 tmp 文件就位到 `blobs_dir/{aa}/{sha256}`, 返回是否由本次调用新建。
///
/// 用 hard_link 而不是 rename: rename 在 Unix 上会静默替换已存在目标,
/// 分不出「新建」与「撞车」; hard_link 撞 AlreadyExists 恰好是去重信号。
/// 正常布局 (文档第 6 章) 下 tmp/ 与 blobs/ 同在数据根, 必同文件系统;
/// 真遇上跨设备 (EXDEV) 就复制到目标目录内的暂存名再链接, 不降级成
/// 直接 copy 到目标 — 那会在撞车时截断别人的完整文件。
fn place_blob(blobs_dir: &Path, tmp: &Path, sha256: &str) -> std::io::Result<bool> {
    let dir = blobs_dir.join(&sha256[..2]);
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(sha256);
    match std::fs::hard_link(tmp, &target) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let staging = dir.join(format!(".ingest-{}", Uuid::new_v4()));
            std::fs::copy(tmp, &staging)?;
            let placed = match std::fs::hard_link(&staging, &target) {
                Ok(()) => true,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => false,
                Err(e) => return Err(e),
            };
            let _ = std::fs::remove_file(&staging);
            Ok(placed)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 盘上布局: {tmp}/blobs (收编目标), {tmp}/tmp (上传暂存, 同文件系统)。
    struct Fixture {
        _tmp: TempDir,
        blobs: PathBuf,
        staging: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let blobs = tmp.path().join("blobs");
        let staging = tmp.path().join("tmp");
        std::fs::create_dir_all(&staging).unwrap();
        Fixture {
            _tmp: tmp,
            blobs,
            staging,
        }
    }

    fn write_tmp(staging: &Path, name: &str, content: &[u8]) -> PathBuf {
        let p = staging.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn hash_file_matches_known_vector() {
        let f = fixture();
        let p = write_tmp(&f.staging, "a", b"hello world");
        let (hex, size) = hash_file(&p).unwrap();
        assert_eq!(
            hex,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(size, 11);
    }

    #[test]
    fn place_blob_dedups_on_disk() {
        let f = fixture();
        let (hex, _) = {
            let p = write_tmp(&f.staging, "one", b"same content");
            hash_file(&p).unwrap()
        };
        let t1 = write_tmp(&f.staging, "t1", b"same content");
        let t2 = write_tmp(&f.staging, "t2", b"same content");
        assert!(place_blob(&f.blobs, &t1, &hex).unwrap());
        assert!(!place_blob(&f.blobs, &t2, &hex).unwrap());
        // 盘上只一份
        let placed = f.blobs.join(&hex[..2]).join(&hex);
        assert_eq!(std::fs::read(&placed).unwrap(), b"same content");
        let count = std::fs::read_dir(placed.parent().unwrap()).unwrap().count();
        assert_eq!(count, 1);
    }

    #[test]
    fn sha256_hex_validation() {
        let good = "a".repeat(64);
        assert!(is_sha256_hex(&good));
        assert!(!is_sha256_hex(&"A".repeat(64)));
        assert!(!is_sha256_hex(&"a".repeat(63)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
    }

    async fn stores(data_root: &Path) -> (crate::Store, BlobStore) {
        let url = std::env::var("YUKIPAN_TEST_DB_URL").expect("缺少 YUKIPAN_TEST_DB_URL");
        let pool = yukipan_db::connect(&url).await.unwrap();
        yukipan_db::migrate(&pool).await.unwrap();
        (
            crate::Store::new(pool.clone()),
            BlobStore::new(pool, data_root.to_path_buf()),
        )
    }

    /// `YUKIPAN_TEST_DB_URL=postgres://... cargo test -p yukipan-store -- --ignored`
    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn blob_ingest_dedup_and_ref_lifecycle() {
        let tmp = TempDir::new().unwrap();
        let data_root = tmp.path().join("data");
        let staging = data_root.join("tmp");
        std::fs::create_dir_all(&staging).unwrap();
        let (store, blobs) = stores(&data_root).await;
        let user = store
            .create_user(&format!("test-{}", Uuid::new_v4()), "pw")
            .await
            .unwrap();

        // dev 库是持久的, 内容带 run 唯一后缀, 重复跑 --ignored 不受历史行影响
        let content = format!("slice-1 content {}", Uuid::new_v4());
        let content = content.as_bytes();

        // 首次收编: 新内容, deduped=false, tmp 被移走
        let t1 = write_tmp(&staging, "u1", content);
        let (sha, _) = hash_file(&t1).unwrap();
        let out = blobs.ingest_blob(&t1, None).await.unwrap();
        assert_eq!(out.sha256, sha);
        assert_eq!(out.size, content.len() as u64);
        assert!(!out.deduped);
        assert!(!t1.exists());
        assert!(blobs.blob_file_path(&sha).exists());

        // 同内容再收编: deduped=true, 盘上仍一份
        let t2 = write_tmp(&staging, "u2", content);
        let out2 = blobs.ingest_blob(&t2, None).await.unwrap();
        assert!(out2.deduped);
        assert!(!t2.exists());
        let aa = data_root.join("blobs").join(&sha[..2]);
        assert_eq!(std::fs::read_dir(&aa).unwrap().count(), 1);

        // 声明 hash 相符通过; 不符拒绝且清掉 tmp
        let t3 = write_tmp(&staging, "u3", content);
        let out3 = blobs.ingest_blob(&t3, Some(&sha)).await.unwrap();
        assert!(out3.deduped);
        let t4 = write_tmp(&staging, "u4", b"other content");
        assert!(matches!(
            blobs.ingest_blob(&t4, Some(&sha)).await,
            Err(StoreError::HashMismatch)
        ));
        assert!(!t4.exists());

        // 指向: 不存在的 blob 拒绝; 正常加/数/删
        let path = LogicalPath::parse("docs/a.txt").unwrap();
        let bogus = "0".repeat(64);
        assert!(matches!(
            blobs.add_private_ref(user.id, &path, &bogus).await,
            Err(StoreError::BlobNotFound)
        ));
        blobs.add_private_ref(user.id, &path, &sha).await.unwrap();
        assert_eq!(blobs.blob_ref_count(&sha).await.unwrap(), 1);
        // 仍被引用时不删
        assert!(!blobs.delete_blob_if_unreferenced(&sha).await.unwrap());
        assert!(blobs.blob_file_path(&sha).exists());
        // 撤指向 → 计数归 0 → 删行 + 删文件
        let removed = blobs.remove_private_ref(user.id, &path).await.unwrap();
        assert_eq!(removed, Some(sha.clone()));
        assert_eq!(blobs.blob_ref_count(&sha).await.unwrap(), 0);
        assert!(blobs.delete_blob_if_unreferenced(&sha).await.unwrap());
        assert!(!blobs.blob_file_path(&sha).exists());
        // 再删返回 false (不存在)
        assert!(!blobs.delete_blob_if_unreferenced(&sha).await.unwrap());
    }

    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL, 设 YUKIPAN_TEST_DB_URL 后加 --ignored 跑"]
    async fn concurrent_ingest_same_content_is_safe() {
        let tmp = TempDir::new().unwrap();
        let data_root = tmp.path().join("data");
        let staging = data_root.join("tmp");
        std::fs::create_dir_all(&staging).unwrap();
        let (_, blobs) = stores(&data_root).await;

        let content = format!("concurrent ingest race {}", Uuid::new_v4());
        let content = content.as_bytes();
        let t1 = write_tmp(&staging, "c1", content);
        let t2 = write_tmp(&staging, "c2", content);
        let (r1, r2) = tokio::join!(blobs.ingest_blob(&t1, None), blobs.ingest_blob(&t2, None));
        let (o1, o2) = (r1.unwrap(), r2.unwrap());
        assert_eq!(o1.sha256, o2.sha256);
        // 恰好一个新建一个去重
        assert_ne!(o1.deduped, o2.deduped);
        let aa = data_root.join("blobs").join(&o1.sha256[..2]);
        assert_eq!(std::fs::read_dir(&aa).unwrap().count(), 1);
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM blobs WHERE sha256 = $1")
            .bind(&o1.sha256)
            .fetch_one(&blobs.pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        // 收尾: 不留历史行 (dev 库持久, 别的用例不应看到这份内容)
        assert!(blobs.delete_blob_if_unreferenced(&o1.sha256).await.unwrap());
    }
}
