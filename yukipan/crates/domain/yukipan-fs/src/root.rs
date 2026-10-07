//! 数据根与根内安全解析。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::LogicalPath;

/// 数据根: 私有区目录树的锚点, 构造时即 canonicalize。
///
/// 解析结果保证落在根内; 错误信息不含真实磁盘路径。
#[derive(Debug, Clone)]
pub struct DataRoot {
    root: PathBuf,
}

/// 根内解析可能遇到的错误。
#[derive(Debug, Error)]
pub enum ResolveError {
    /// 目标或其中间祖先不存在。
    #[error("路径不存在")]
    NotFound,
    /// 中间分量不是目录, 无法继续向下。
    #[error("路径中间分量不是目录")]
    NotADirectory,
    /// 经符号链接解析后逃出了数据根。
    #[error("路径逃出数据根")]
    Escape,
    /// 底层 IO 错误 (权限、竞争删除等)。
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl DataRoot {
    /// 以 `root` 为数据根。目录必须已存在 (由 ops/bootstrap-host.sh 建立)。
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ResolveError> {
        let root = match root.as_ref().canonicalize() {
            Ok(abs) => abs,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(ResolveError::NotFound),
            Err(e) => return Err(ResolveError::Io(e)),
        };
        Ok(Self { root })
    }

    /// 数据根的绝对路径 (已 canonicalize)。
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// 解析一个**已存在**的逻辑路径, 返回根内绝对路径。
    pub fn resolve_existing(&self, path: &LogicalPath) -> Result<PathBuf, ResolveError> {
        self.canonicalize_checked(&self.root.join(path.as_rel_path()))
    }

    /// 解析一个**可能不存在**的逻辑路径 (创建目标):
    /// 最深的已存在祖先必须在根内, 不存在的余下分量直接拼接。
    pub fn resolve_for_create(&self, path: &LogicalPath) -> Result<PathBuf, ResolveError> {
        let mut existing = self.root.clone();
        let mut pending: Vec<&str> = Vec::new();
        for comp in path.components() {
            if !pending.is_empty() {
                pending.push(comp);
                continue;
            }
            let candidate = existing.join(comp);
            match std::fs::symlink_metadata(&candidate) {
                Ok(_) => existing = candidate,
                Err(e) if e.kind() == ErrorKind::NotFound => pending.push(comp),
                Err(e) if e.kind() == ErrorKind::NotADirectory => {
                    return Err(ResolveError::NotADirectory);
                }
                Err(e) => return Err(ResolveError::Io(e)),
            }
        }
        let mut resolved = self.canonicalize_checked(&existing)?;
        for comp in pending {
            resolved.push(comp);
        }
        Ok(resolved)
    }

    /// canonicalize 并校验结果仍在根内。
    fn canonicalize_checked(&self, path: &Path) -> Result<PathBuf, ResolveError> {
        let abs = match path.canonicalize() {
            Ok(abs) => abs,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(ResolveError::NotFound),
            Err(e) if e.kind() == ErrorKind::NotADirectory => {
                return Err(ResolveError::NotADirectory);
            }
            Err(e) => return Err(ResolveError::Io(e)),
        };
        if abs.starts_with(&self.root) {
            Ok(abs)
        } else {
            Err(ResolveError::Escape)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    struct Fixture {
        _tmp: TempDir,
        root: DataRoot,
        root_path: PathBuf,
        outside: PathBuf,
    }

    /// 临时目录布局: {tmp}/private/u1 (数据根), {tmp}/outside (根外目录)。
    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        let root_dir = base.join("private/u1");
        fs::create_dir_all(&root_dir).unwrap();
        let outside = base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        let root = DataRoot::new(&root_dir).unwrap();
        let root_path = root.path().to_path_buf();
        Fixture {
            _tmp: tmp,
            root,
            root_path,
            outside,
        }
    }

    fn lp(input: &str) -> LogicalPath {
        LogicalPath::parse(input).unwrap()
    }

    #[test]
    fn new_requires_existing_dir() {
        let tmp = TempDir::new().unwrap();
        let err = DataRoot::new(tmp.path().join("missing")).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound));
    }

    #[test]
    fn resolve_existing_file() {
        let f = fixture();
        fs::create_dir_all(f.root_path.join("docs")).unwrap();
        fs::write(f.root_path.join("docs/a.txt"), "hi").unwrap();
        let abs = f.root.resolve_existing(&lp("docs/a.txt")).unwrap();
        assert_eq!(abs, f.root_path.join("docs/a.txt"));
    }

    #[test]
    fn resolve_existing_root() {
        let f = fixture();
        let abs = f.root.resolve_existing(&LogicalPath::root()).unwrap();
        assert_eq!(abs, f.root_path);
    }

    #[test]
    fn resolve_existing_missing() {
        let f = fixture();
        let err = f.root.resolve_existing(&lp("nope.txt")).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound));
    }

    #[test]
    fn resolve_existing_symlink_out_is_escape() {
        let f = fixture();
        symlink(&f.outside, f.root_path.join("link")).unwrap();
        // 解析链接本身与其内部目标, 都不得出根。
        let err = f.root.resolve_existing(&lp("link")).unwrap_err();
        assert!(matches!(err, ResolveError::Escape));
        let err = f.root.resolve_existing(&lp("link/secret.txt")).unwrap_err();
        assert!(matches!(err, ResolveError::Escape));
    }

    #[test]
    fn resolve_existing_symlink_inside_is_allowed() {
        let f = fixture();
        fs::create_dir_all(f.root_path.join("real/dir")).unwrap();
        fs::write(f.root_path.join("real/dir/x"), "x").unwrap();
        symlink(f.root_path.join("real"), f.root_path.join("alias")).unwrap();
        let abs = f.root.resolve_existing(&lp("alias/dir/x")).unwrap();
        assert_eq!(abs, f.root_path.join("real/dir/x"));
    }

    #[test]
    fn create_appends_missing_components() {
        let f = fixture();
        let abs = f.root.resolve_for_create(&lp("new/deep/file.txt")).unwrap();
        assert_eq!(abs, f.root_path.join("new/deep/file.txt"));
    }

    #[test]
    fn create_root_is_root_itself() {
        let f = fixture();
        let abs = f.root.resolve_for_create(&LogicalPath::root()).unwrap();
        assert_eq!(abs, f.root_path);
    }

    #[test]
    fn create_existing_resolves_like_existing() {
        let f = fixture();
        fs::write(f.root_path.join("a.txt"), "a").unwrap();
        let abs = f.root.resolve_for_create(&lp("a.txt")).unwrap();
        assert_eq!(abs, f.root_path.join("a.txt"));
    }

    #[test]
    fn create_under_symlink_out_is_escape() {
        let f = fixture();
        symlink(&f.outside, f.root_path.join("link")).unwrap();
        let err = f.root.resolve_for_create(&lp("link/evil.txt")).unwrap_err();
        assert!(matches!(err, ResolveError::Escape));
    }

    #[test]
    fn create_through_file_is_not_a_directory() {
        let f = fixture();
        fs::write(f.root_path.join("file.txt"), "f").unwrap();
        let err = f
            .root
            .resolve_for_create(&lp("file.txt/child"))
            .unwrap_err();
        assert!(matches!(err, ResolveError::NotADirectory));
    }
}
