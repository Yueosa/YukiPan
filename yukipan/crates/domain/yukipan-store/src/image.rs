//! 图床域 (文档第 3、6 章): 相册、图片指向、标签。
//!
//! 图床没有目录树, 列表以库为准; 盘上的公开文件 (public/images/<public_name>)
//! 与指向行的生命周期由 API 层组合维护。这些方法全是纯库操作, 挂在 [`Store`] 上。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{Result, Store, StoreError, sql_safe};

/// 默认相册名 (上传/秒传/分享不指定相册时落入, 懒建)。
pub const DEFAULT_ALBUM_NAME: &str = "默认相册";

/// 相册视图 (管理侧)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub is_default: bool,
}

/// 相册摘要 (公开列表用): 带图片数与封面公开名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumSummary {
    pub id: Uuid,
    pub name: String,
    pub is_default: bool,
    pub image_count: u64,
    /// 封面图的 public_name (册里最新一张), 空册为 None。
    pub cover: Option<String>,
}

/// 标签摘要 (公开列表用): 带使用次数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSummary {
    pub id: Uuid,
    pub name: String,
    pub count: u64,
}

/// 图片指向 (墙上的一张图)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub id: Uuid,
    pub album_id: Uuid,
    pub album_name: String,
    pub public_name: String,
    pub orig_name: String,
    pub sha256: String,
    pub size: u64,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// 标签名规范化: trim + 折叠内部空白 + 小写 (文档第 6 章「名字规范化后唯一」)。
pub fn normalize_tag_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// image_refs 联查的公共 SELECT (相册名、大小、标签数组都带上)。
const IMAGE_SELECT: &str = "
    SELECT i.id, i.album_id, a.name, i.public_name, i.orig_name, i.sha256, b.size,
           COALESCE((SELECT array_agg(t.name ORDER BY t.name)
                     FROM image_tags it JOIN tags t ON t.id = it.tag_id
                     WHERE it.image_id = i.id), '{}'),
           i.created_at
    FROM image_refs i
    JOIN albums a ON a.id = i.album_id
    JOIN blobs b ON b.sha256 = i.sha256";

type ImageRow = (Uuid, Uuid, String, String, String, String, i64, Vec<String>, DateTime<Utc>);

fn row_to_image(r: ImageRow) -> ImageRef {
    ImageRef {
        id: r.0,
        album_id: r.1,
        album_name: r.2,
        public_name: r.3,
        orig_name: r.4,
        sha256: r.5,
        size: r.6 as u64,
        tags: r.7,
        created_at: r.8,
    }
}

impl Store {
    /// 建相册。重名报 [`StoreError::AlbumNameTaken`]。
    pub async fn create_album(&self, user_id: Uuid, name: &str) -> Result<Album> {
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, bool)>(
            "INSERT INTO albums (user_id, name) VALUES ($1, $2)
             RETURNING id, user_id, name, is_default",
        )
        .bind(user_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(db) = &e
                && db.is_unique_violation()
            {
                return StoreError::AlbumNameTaken;
            }
            StoreError::Db(e)
        })?;
        Ok(Album {
            id: row.0,
            user_id: row.1,
            name: row.2,
            is_default: row.3,
        })
    }

    /// 取默认相册, 没有就懒建 (首次上传/分享时)。
    ///
    /// 并发: 两个请求同时发现没有默认册会同插 `默认相册`, 靠 UNIQUE(user_id, name)
    /// 兜住 — 输家 ON CONFLICT 落空后重新 SELECT 到赢家那行。若用户手工建过同名
    /// 非默认册, 就把它扶正为默认册。
    pub async fn ensure_default_album(&self, user_id: Uuid) -> Result<Album> {
        if let Some(a) = self.find_album(user_id, true).await? {
            return Ok(a);
        }
        sqlx::query(
            "INSERT INTO albums (user_id, name, is_default) VALUES ($1, $2, true)
             ON CONFLICT (user_id, name) DO NOTHING",
        )
        .bind(user_id)
        .bind(DEFAULT_ALBUM_NAME)
        .execute(&self.pool)
        .await?;
        let album = self
            .find_album_by_name(user_id, DEFAULT_ALBUM_NAME)
            .await?
            .expect("刚插入或并发插入的默认相册必然存在");
        if album.is_default {
            return Ok(album);
        }
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, bool)>(
            "UPDATE albums SET is_default = true WHERE id = $1
             RETURNING id, user_id, name, is_default",
        )
        .bind(album.id)
        .fetch_one(&self.pool)
        .await?;
        Ok(Album {
            id: row.0,
            user_id: row.1,
            name: row.2,
            is_default: row.3,
        })
    }

    async fn find_album(&self, user_id: Uuid, is_default: bool) -> Result<Option<Album>> {
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, bool)>(
            "SELECT id, user_id, name, is_default FROM albums
             WHERE user_id = $1 AND is_default = $2 LIMIT 1",
        )
        .bind(user_id)
        .bind(is_default)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| Album {
            id: r.0,
            user_id: r.1,
            name: r.2,
            is_default: r.3,
        }))
    }

    async fn find_album_by_name(&self, user_id: Uuid, name: &str) -> Result<Option<Album>> {
        let row = sqlx::query_as::<_, (Uuid, Uuid, String, bool)>(
            "SELECT id, user_id, name, is_default FROM albums
             WHERE user_id = $1 AND name = $2",
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| Album {
            id: r.0,
            user_id: r.1,
            name: r.2,
            is_default: r.3,
        }))
    }

    /// 公开: 全部相册 (单用户应用, 就是 owner 的册), 带图片数与封面。
    pub async fn list_albums(&self) -> Result<Vec<AlbumSummary>> {
        let rows = sqlx::query_as::<_, (Uuid, String, bool, i64, Option<String>)>(
            "SELECT a.id, a.name, a.is_default, COUNT(i.id),
                    (SELECT i2.public_name FROM image_refs i2
                     WHERE i2.album_id = a.id ORDER BY i2.created_at DESC LIMIT 1)
             FROM albums a LEFT JOIN image_refs i ON i.album_id = a.id
             GROUP BY a.id ORDER BY a.created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| AlbumSummary {
                id: r.0,
                name: r.1,
                is_default: r.2,
                image_count: r.3 as u64,
                cover: r.4,
            })
            .collect())
    }

    /// 删相册 (限本人): 默认册报 [`StoreError::DefaultAlbumForbidden`],
    /// 非空册报 [`StoreError::AlbumNotEmpty`], 不存在/别人的报 [`StoreError::AlbumNotFound`]。
    pub async fn delete_album(&self, user_id: Uuid, album_id: Uuid) -> Result<()> {
        let row: Option<(bool,)> =
            sqlx::query_as("SELECT is_default FROM albums WHERE id = $1 AND user_id = $2")
                .bind(album_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some((is_default,)) = row else {
            return Err(StoreError::AlbumNotFound);
        };
        if is_default {
            return Err(StoreError::DefaultAlbumForbidden);
        }
        let (n,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM image_refs WHERE album_id = $1")
                .bind(album_id)
                .fetch_one(&self.pool)
                .await?;
        if n > 0 {
            return Err(StoreError::AlbumNotEmpty);
        }
        sqlx::query("DELETE FROM albums WHERE id = $1")
            .bind(album_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 公开: 全部标签 (单用户应用), 带使用次数, 给墙上筛选用。
    pub async fn list_tags(&self) -> Result<Vec<TagSummary>> {
        let rows = sqlx::query_as::<_, (Uuid, String, i64)>(
            "SELECT t.id, t.name, COUNT(it.image_id)
             FROM tags t LEFT JOIN image_tags it ON it.tag_id = t.id
             GROUP BY t.id ORDER BY t.name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| TagSummary {
                id: r.0,
                name: r.1,
                count: r.2 as u64,
            })
            .collect())
    }

    /// 该用户墙上是否已有此 hash 的图 (任一相册) — 上传去重判据 (文档第 6 章:
    /// 图床里已有同一 hash, 直接回那张图, 不新建、不加配额)。
    pub async fn find_image_by_hash(
        &self,
        user_id: Uuid,
        sha256: &str,
    ) -> Result<Option<ImageRef>> {
        let row = sqlx::query_as::<_, ImageRow>(
            sql_safe(format!("{IMAGE_SELECT} WHERE a.user_id = $1 AND i.sha256 = $2 LIMIT 1")),
        )
        .bind(user_id)
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_image))
    }

    /// 该用户在指定相册里是否已有此 hash 的图 — 秒传/分享的「同册回旧图,
    /// 换册挂新指向」判据 (文档第 6 章)。
    pub async fn find_image_in_album(
        &self,
        user_id: Uuid,
        album_id: Uuid,
        sha256: &str,
    ) -> Result<Option<ImageRef>> {
        let row = sqlx::query_as::<_, ImageRow>(
            sql_safe(format!("{IMAGE_SELECT} WHERE a.user_id = $1 AND i.album_id = $2 AND i.sha256 = $3 LIMIT 1")),
        )
        .bind(user_id)
        .bind(album_id)
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_image))
    }

    /// 校验相册归属 (挂图前)。
    pub async fn album_owned(&self, user_id: Uuid, album_id: Uuid) -> Result<bool> {
        let (yes,): (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM albums WHERE id = $1 AND user_id = $2)")
                .bind(album_id)
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(yes)
    }

    /// 插一条图片指向。public_name 由调用方 (API 层) 生成, 与盘上公开文件同名。
    pub async fn insert_image(
        &self,
        album_id: Uuid,
        public_name: &str,
        orig_name: &str,
        sha256: &str,
    ) -> Result<ImageRef> {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO image_refs (album_id, public_name, orig_name, sha256)
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(album_id)
        .bind(public_name)
        .bind(orig_name)
        .bind(sha256)
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
        Ok(self.get_image(id).await?.expect("刚插入的图必然存在"))
    }

    /// 按 id 取图 (不限归属, 内部用)。
    pub async fn get_image(&self, image_id: Uuid) -> Result<Option<ImageRef>> {
        let row =
            sqlx::query_as::<_, ImageRow>(sql_safe(format!("{IMAGE_SELECT} WHERE i.id = $1")))
                .bind(image_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(row_to_image))
    }

    /// 公开: 图片墙列表, 可按相册/标签 (规范化名) 筛, offset 分页, 新的在前。
    /// 返回 (本页, 总数)。
    pub async fn list_images(
        &self,
        album_id: Option<Uuid>,
        tag_norm: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<ImageRef>, u64)> {
        // 两个过滤条件都可选, 用「参数 IS NULL 不过滤」写法保持单条静态 SQL。
        const FILTER: &str = "($1::uuid IS NULL OR i.album_id = $1)
            AND ($2::text IS NULL OR EXISTS(
                SELECT 1 FROM image_tags it JOIN tags t ON t.id = it.tag_id
                WHERE it.image_id = i.id AND t.name_norm = $2))";
        let (total,): (i64,) = sqlx::query_as(sql_safe(format!(
            "SELECT COUNT(*) FROM image_refs i WHERE {FILTER}"
        )))
        .bind(album_id)
        .bind(tag_norm)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query_as::<_, ImageRow>(sql_safe(format!(
            "{IMAGE_SELECT} WHERE {FILTER} ORDER BY i.created_at DESC, i.id DESC
             LIMIT $3 OFFSET $4"
        )))
        .bind(album_id)
        .bind(tag_norm)
        .bind(i64::from(per_page))
        .bind(i64::from(page.saturating_sub(1)) * i64::from(per_page))
        .fetch_all(&self.pool)
        .await?;
        Ok((rows.into_iter().map(row_to_image).collect(), total as u64))
    }

    /// 删图片指向 (限本人), 返回被删的行 (调用方做盘上/账本/blob 收尾)。
    /// 不存在或别人的返回 [`StoreError::ImageNotFound`]。
    pub async fn delete_image(&self, user_id: Uuid, image_id: Uuid) -> Result<ImageRef> {
        let image = self.get_image(image_id).await?;
        let Some(image) = image else {
            return Err(StoreError::ImageNotFound);
        };
        if !self.album_owned(user_id, image.album_id).await? {
            return Err(StoreError::ImageNotFound);
        }
        sqlx::query("DELETE FROM image_refs WHERE id = $1")
            .bind(image_id)
            .execute(&self.pool)
            .await?;
        Ok(image)
    }

    /// 全量覆盖一张图的标签 (限本人)。不存在的规范化名自动建 tags 行;
    /// 返回覆盖后的标签名列表 (按规范化去重后)。
    pub async fn set_image_tags(
        &self,
        user_id: Uuid,
        image_id: Uuid,
        names: &[String],
    ) -> Result<Vec<String>> {
        let image = self.get_image(image_id).await?;
        let Some(image) = image else {
            return Err(StoreError::ImageNotFound);
        };
        if !self.album_owned(user_id, image.album_id).await? {
            return Err(StoreError::ImageNotFound);
        }

        // 先按规范化名去重; 展示名保留原大小写但折叠空白, 空名 (全空白) 由 API 层挡掉。
        let mut deduped: Vec<(String, String)> = Vec::new();
        for name in names {
            let display = name.split_whitespace().collect::<Vec<_>>().join(" ");
            let norm = display.to_lowercase();
            if !deduped.iter().any(|(_, n)| *n == norm) {
                deduped.push((display, norm));
            }
        }

        // 覆盖是「删旧关联 + get-or-create 标签 + 挂新关联」三步, 必须一起成/败。
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM image_tags WHERE image_id = $1")
            .bind(image_id)
            .execute(&mut *tx)
            .await?;
        let mut result = Vec::with_capacity(deduped.len());
        for (display, norm) in deduped {
            let (tag_id,): (Uuid,) = sqlx::query_as(
                "INSERT INTO tags (user_id, name, name_norm) VALUES ($1, $2, $3)
                 ON CONFLICT (user_id, name_norm) DO UPDATE SET name = EXCLUDED.name
                 RETURNING id",
            )
            .bind(user_id)
            .bind(&display)
            .bind(&norm)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO image_tags (image_id, tag_id) VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
            )
            .bind(image_id)
            .bind(tag_id)
            .execute(&mut *tx)
            .await?;
            result.push(display);
        }
        tx.commit().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_name_normalization() {
        assert_eq!(normalize_tag_name("  旅行  2024 "), "旅行 2024");
        assert_eq!(normalize_tag_name("Hello  World"), "hello world");
        assert_eq!(normalize_tag_name("风景"), "风景");
        assert_eq!(normalize_tag_name("\tA\tB\t"), "a b");
        assert_eq!(normalize_tag_name("   "), "");
    }
}
