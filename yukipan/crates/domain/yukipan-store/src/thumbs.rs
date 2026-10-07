//! 图床缩略图 (图片墙加载优化): 长边 640, webp q80, 落 `public/thumbs/<uuid>.webp`。
//!
//! 生成是尽力而为: 解码失败 (坏图、avif — 未引 avif 解码器) 不阻塞挂图主流程,
//! 仅 warn 日志, thumb 缺失时 API 回 thumb_url=null 由前端回退原图。

use std::path::{Path, PathBuf};

use tokio::task::spawn_blocking;
use tracing::warn;

use crate::BlobStore;

/// 缩略图长边像素。
pub const THUMB_MAX_EDGE: u32 = 640;
/// webp 编码质量。
const THUMB_QUALITY: u8 = 80;

impl BlobStore {
    /// 缩略图盘上路径: `public/thumbs/<public_name 去扩展名>.webp`。
    pub fn thumb_path(&self, public_name: &str) -> PathBuf {
        self.data_root()
            .join("public/thumbs")
            .join(thumb_file_name(public_name))
    }

    /// 缩略图公开 url (文件存在才给); 不存在返回 None, 前端回退原图。
    pub fn thumb_url(&self, public_name: &str) -> Option<String> {
        self.thumb_path(public_name)
            .is_file()
            .then(|| format!("/public/thumbs/{}", thumb_file_name(public_name)))
    }

    /// 从源文件 (通常是 blob 文件) 生成缩略图。spawn_blocking 里做, 单张有界;
    /// 失败仅 warn 并返回 false。
    pub async fn generate_thumb(&self, source: &Path, public_name: &str) -> bool {
        let source = source.to_path_buf();
        let target = self.thumb_path(public_name);

        let result = spawn_blocking(move || make_thumb(&source, &target))
            .await
            .expect("缩略图线程 panic");
        match result {
            Ok(()) => true,
            Err(e) => {
                warn!("生成缩略图失败 ({public_name}): {e}");
                false
            }
        }
    }

    /// 删缩略图 (删指向时连带); 不存在不算错。
    pub fn remove_thumb(&self, public_name: &str) {
        if let Err(e) = std::fs::remove_file(self.thumb_path(public_name))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            warn!("删缩略图失败 ({public_name}): {e}");
        }
    }
}

/// 缩略图文件名: 公开名去扩展名 + .webp (public_name 是 server 生成的 `<uuid>.<ext>`)。
fn thumb_file_name(public_name: &str) -> String {
    let stem = Path::new(public_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| public_name.to_string());
    format!("{stem}.webp")
}

/// 解码 → 长边缩到 [`THUMB_MAX_EDGE`] → lossy webp q80。先写临时名再 rename,
/// 崩溃不留半截缩略图。blob 文件无扩展名, 必须用 with_guessed_format 按
/// 内容 sniff (image::open 在 0.24 默认按扩展名判格式, 会误判 Unknown)。
/// new_with_quality 在 image 0.25 被移除, 这是 workspace 锁 0.24.9 的原因。
#[allow(deprecated)]
fn make_thumb(source: &Path, target: &Path) -> image::ImageResult<()> {
    let img = image::io::Reader::open(source)?
        .with_guessed_format()?
        .decode()?;
    let thumb = img.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let staging = target.with_extension("webp.tmp");
    let mut out = std::fs::File::create(&staging)?;
    let encoder = image::codecs::webp::WebPEncoder::new_with_quality(
        &mut out,
        image::codecs::webp::WebPQuality::lossy(THUMB_QUALITY),
    );
    thumb.write_with_encoder(encoder)?;
    drop(out);
    std::fs::rename(&staging, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 造一张 1000x500 的 png (save 按扩展名选格式, 这里显式 write_to, 与扩展名无关)。
    fn write_png(path: &Path) {
        let img = image::RgbImage::from_fn(1000, 500, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        img.write_to(
            &mut std::io::BufWriter::new(std::fs::File::create(path).unwrap()),
            image::ImageFormat::Png,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn generate_resize_and_remove() {
        let tmp = TempDir::new().unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/none")
            .unwrap();
        let blobs = BlobStore::new(pool, tmp.path());
        // blob 文件没有扩展名, 测试也按无扩展名来 (防止按扩展名判格式的回归)
        let src = tmp.path().join("src-noext");
        write_png(&src);

        assert!(blobs.generate_thumb(&src, "abcd1234.png").await);
        let thumb = blobs.thumb_path("abcd1234.png");
        assert!(thumb.exists());
        let decoded = image::open(&thumb).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (640, 320));
        assert_eq!(
            blobs.thumb_url("abcd1234.png").as_deref(),
            Some("/public/thumbs/abcd1234.webp")
        );

        blobs.remove_thumb("abcd1234.png");
        assert!(!thumb.exists());
        assert_eq!(blobs.thumb_url("abcd1234.png"), None);
        // 不存在不算错
        blobs.remove_thumb("abcd1234.png");
    }

    #[tokio::test]
    async fn bad_source_does_not_block() {
        let tmp = TempDir::new().unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/none")
            .unwrap();
        let blobs = BlobStore::new(pool, tmp.path());
        let src = tmp.path().join("bad.png");
        std::fs::write(&src, b"not an image").unwrap();
        assert!(!blobs.generate_thumb(&src, "xyz.png").await);
        assert!(!blobs.thumb_path("xyz.png").exists());
    }
}

#[cfg(test)]
mod repro {
    #[test]
    fn rgbimage_write_to_png_roundtrip() {
        let img = image::RgbImage::from_fn(1000, 500, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 96])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let bytes = buf.into_inner();
        println!("png bytes: {}, magic: {:?}", bytes.len(), &bytes[..8]);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1000, 500));
    }
}
