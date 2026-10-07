//! 私有区目录树操作 (文档第 2、5、7 章): 列目录 / 建目录 / 移动 / 删除。
//!
//! 这些操作只碰文件系统。文件的指向记录 (private_refs) 由 store 侧维护,
//! API 层负责把两边组合成事务般的一致行为 (删除文件 → 删指向 → 引用计数)。
//!
//! 安全约定:
//! - 所有路径先经 [`DataRoot`] 解析, 错误信息不含真实磁盘路径;
//! - 列目录遇到符号链接条目直接跳过 (不跟随也不展示), 与第 5 章
//!   「不许顺着软链逃出数据根」的口径一致 — 区目录里正常只会有 hardlink,
//!   出现 symlink 本身就是异常, 当作不存在处理;
//! - 移动/删除作用于「叶子本身」而不是它指向的目标: 末端分量是符号链接时
//!   操作的是链接, 不会被 canonicalize 穿到目标上。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{DataRoot, LogicalPath, ResolveError};

/// 目录条目 (list_dir 的单行)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// 条目名 (单个分量, 不含路径)。
    pub name: String,
    /// 是否目录 (符号链接已被跳过, 不会出现)。
    pub is_dir: bool,
    /// 字节大小 (目录为 0)。
    pub size: u64,
    /// 最后修改时间; 取不到 (竞争删除等) 时为 None。
    pub modified: Option<SystemTime>,
}

impl DataRoot {
    /// 列目录, 不递归。返回目录在前、各自按名字排序的条目。
    pub fn list_dir(&self, path: &LogicalPath) -> Result<Vec<DirEntry>, ResolveError> {
        let abs = self.resolve_existing(path)?;
        let read = fs::read_dir(&abs).map_err(map_io)?;
        let mut entries = Vec::new();
        for item in read {
            let item = item.map_err(ResolveError::Io)?;
            // symlink_metadata 不跟随末端链接; 是链接就跳过 (见模块文档)。
            let meta = match item.file_type() {
                Ok(t) if t.is_symlink() => continue,
                Ok(_) => item.metadata().map_err(ResolveError::Io)?,
                Err(e) => return Err(ResolveError::Io(e)),
            };
            let is_dir = meta.is_dir();
            entries.push(DirEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                is_dir,
                size: if is_dir { 0 } else { meta.len() },
                modified: meta.modified().ok(),
            });
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        Ok(entries)
    }

    /// 按层创建目录 (已存在的部分忽略)。
    pub fn create_dir_all(&self, path: &LogicalPath) -> Result<(), ResolveError> {
        let abs = self.resolve_for_create(path)?;
        fs::create_dir_all(&abs).map_err(map_io)
    }

    /// 移动/改名: `to` 含新名字, 中间目录不代建 (父目录必须已存在), 不覆盖已有目标。
    pub fn move_entry(&self, from: &LogicalPath, to: &LogicalPath) -> Result<(), ResolveError> {
        let src = self.resolve_leaf(from)?;
        fs::symlink_metadata(&src).map_err(map_io)?;
        let dst = self.resolve_leaf(to)?;
        if fs::symlink_metadata(&dst).is_ok() {
            // 不静默覆盖。这里与 rename 之间仍有 TOCTOU 窗口: 真撞上时
            // rename 会覆盖同刻落进来的条目, 接受这个竞态 (单用户私有区)。
            return Err(ResolveError::AlreadyExists);
        }
        fs::rename(&src, &dst).map_err(map_io)
    }

    /// 删除文件或目录。目录非空且 `recursive = false` 时报 [`ResolveError::NotEmpty`]。
    pub fn remove_entry(&self, path: &LogicalPath, recursive: bool) -> Result<(), ResolveError> {
        let abs = self.resolve_leaf(path)?;
        let meta = fs::symlink_metadata(&abs).map_err(map_io)?;
        if meta.is_dir() {
            if recursive {
                fs::remove_dir_all(&abs).map_err(map_io)
            } else {
                fs::remove_dir(&abs).map_err(|e| {
                    if e.kind() == ErrorKind::DirectoryNotEmpty {
                        ResolveError::NotEmpty
                    } else {
                        map_io(e)
                    }
                })
            }
        } else {
            // 文件与符号链接都走 remove_file: 删链接本身, 不碰目标。
            fs::remove_file(&abs).map_err(map_io)
        }
    }

    /// 在私有区树内为 `target` (数据根内某 blob 的绝对路径) 建 hardlink。
    ///
    /// 安全不变量管的是「链接落在根内」: `path` 过 resolve_for_create,
    /// 中间符号链接不得逃出根; `target` 本身必须是已存在的普通文件。
    /// 目标位置已有条目 (含符号链接) 报 [`ResolveError::AlreadyExists`] — 不覆盖。
    pub fn link_file(&self, path: &LogicalPath, target: &Path) -> Result<(), ResolveError> {
        let abs = self.resolve_for_create(path)?;
        if fs::symlink_metadata(&abs).is_ok() {
            return Err(ResolveError::AlreadyExists);
        }
        let meta = fs::metadata(target).map_err(map_io)?;
        if !meta.is_file() {
            return Err(ResolveError::NotFound);
        }
        fs::hard_link(target, &abs).map_err(map_io)
    }

    /// 解析「叶子路径」: 父目录过 [`DataRoot::resolve_existing`] (必须在根内且是目录),
    /// 末端分量直接拼接 — 不 canonicalize 末端, 符号链接按链接本身操作。
    fn resolve_leaf(&self, path: &LogicalPath) -> Result<PathBuf, ResolveError> {
        let name = path.file_name().ok_or(ResolveError::RootForbidden)?;
        let parent = path.parent().expect("非根路径必有父");
        let dir = self.resolve_existing(&parent)?;
        if !dir.is_dir() {
            return Err(ResolveError::NotADirectory);
        }
        Ok(dir.join(name))
    }
}

/// IO 错误归类: 缺路径/中间分量非目录映射成语义变体, 其余透传。
fn map_io(e: std::io::Error) -> ResolveError {
    match e.kind() {
        ErrorKind::NotFound => ResolveError::NotFound,
        ErrorKind::NotADirectory => ResolveError::NotADirectory,
        _ => ResolveError::Io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    struct Fixture {
        _tmp: TempDir,
        root: DataRoot,
        root_path: PathBuf,
        outside: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let root_dir = tmp.path().join("private/u1");
        fs::create_dir_all(&root_dir).unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
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
    fn list_dirs_first_then_sorted() {
        let f = fixture();
        fs::create_dir_all(f.root_path.join("zdir")).unwrap();
        fs::create_dir_all(f.root_path.join("adir")).unwrap();
        fs::write(f.root_path.join("b.txt"), "bb").unwrap();
        fs::write(f.root_path.join("a.txt"), "a").unwrap();
        let entries = f.root.list_dir(&LogicalPath::root()).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["adir", "zdir", "a.txt", "b.txt"]);
        assert!(entries[0].is_dir && entries[1].is_dir);
        assert!(!entries[2].is_dir);
        assert_eq!(entries[2].size, 1);
        assert_eq!(entries[3].size, 2);
        assert!(entries.iter().all(|e| e.modified.is_some()));
    }

    #[test]
    fn list_skips_symlinks() {
        let f = fixture();
        fs::write(f.root_path.join("real.txt"), "x").unwrap();
        symlink(f.root_path.join("real.txt"), f.root_path.join("link")).unwrap();
        symlink(&f.outside, f.root_path.join("escape")).unwrap();
        let entries = f.root.list_dir(&LogicalPath::root()).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["real.txt"]);
    }

    #[test]
    fn list_file_is_not_a_directory() {
        let f = fixture();
        fs::write(f.root_path.join("a.txt"), "a").unwrap();
        let err = f.root.list_dir(&lp("a.txt")).unwrap_err();
        assert!(matches!(err, ResolveError::NotADirectory));
    }

    #[test]
    fn mkdir_nested_and_idempotent() {
        let f = fixture();
        f.root.create_dir_all(&lp("a/b/c")).unwrap();
        assert!(f.root_path.join("a/b/c").is_dir());
        f.root.create_dir_all(&lp("a/b/c")).unwrap();
        f.root.create_dir_all(&LogicalPath::root()).unwrap();
    }

    #[test]
    fn move_rename_and_cross_dir() {
        let f = fixture();
        fs::create_dir_all(f.root_path.join("dst")).unwrap();
        fs::write(f.root_path.join("a.txt"), "hello").unwrap();
        f.root.move_entry(&lp("a.txt"), &lp("b.txt")).unwrap();
        assert!(!f.root_path.join("a.txt").exists());
        assert_eq!(fs::read(f.root_path.join("b.txt")).unwrap(), b"hello");
        f.root.move_entry(&lp("b.txt"), &lp("dst/c.txt")).unwrap();
        assert!(f.root_path.join("dst/c.txt").exists());
    }

    #[test]
    fn move_rejects_missing_parent() {
        let f = fixture();
        fs::write(f.root_path.join("a.txt"), "a").unwrap();
        let err = f.root.move_entry(&lp("a.txt"), &lp("nope/b.txt")).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound));
    }

    #[test]
    fn move_rejects_existing_target_and_root() {
        let f = fixture();
        fs::write(f.root_path.join("a.txt"), "a").unwrap();
        fs::write(f.root_path.join("b.txt"), "b").unwrap();
        let err = f.root.move_entry(&lp("a.txt"), &lp("b.txt")).unwrap_err();
        assert!(matches!(err, ResolveError::AlreadyExists));
        assert_eq!(fs::read(f.root_path.join("b.txt")).unwrap(), b"b");
        let err = f.root.move_entry(&LogicalPath::root(), &lp("x")).unwrap_err();
        assert!(matches!(err, ResolveError::RootForbidden));
    }

    #[test]
    fn move_symlink_moves_link_not_target() {
        let f = fixture();
        fs::create_dir_all(f.root_path.join("dst")).unwrap();
        fs::write(f.root_path.join("real.txt"), "x").unwrap();
        symlink(f.root_path.join("real.txt"), f.root_path.join("link")).unwrap();
        f.root.move_entry(&lp("link"), &lp("dst/link")).unwrap();
        assert!(f.root_path.join("real.txt").exists());
        assert!(fs::symlink_metadata(f.root_path.join("dst/link")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn remove_file_and_dir() {
        let f = fixture();
        fs::write(f.root_path.join("a.txt"), "a").unwrap();
        f.root.remove_entry(&lp("a.txt"), false).unwrap();
        assert!(!f.root_path.join("a.txt").exists());

        fs::create_dir_all(f.root_path.join("empty")).unwrap();
        f.root.remove_entry(&lp("empty"), false).unwrap();
        assert!(!f.root_path.join("empty").exists());
    }

    #[test]
    fn remove_nonempty_dir_requires_recursive() {
        let f = fixture();
        fs::create_dir_all(f.root_path.join("d/sub")).unwrap();
        fs::write(f.root_path.join("d/f.txt"), "f").unwrap();
        let err = f.root.remove_entry(&lp("d"), false).unwrap_err();
        assert!(matches!(err, ResolveError::NotEmpty));
        f.root.remove_entry(&lp("d"), true).unwrap();
        assert!(!f.root_path.join("d").exists());
    }

    #[test]
    fn remove_root_is_forbidden() {
        let f = fixture();
        for recursive in [false, true] {
            let err = f.root.remove_entry(&LogicalPath::root(), recursive).unwrap_err();
            assert!(matches!(err, ResolveError::RootForbidden));
        }
    }

    #[test]
    fn link_file_hardlinks_blob() {
        let f = fixture();
        let blob = f.outside.join("blob1");
        fs::write(&blob, "content").unwrap();
        fs::create_dir_all(f.root_path.join("docs")).unwrap();
        f.root.link_file(&lp("docs/a.txt"), &blob).unwrap();
        // 同一份内容两个名字, inode 级共享
        assert_eq!(fs::read(f.root_path.join("docs/a.txt")).unwrap(), b"content");
        assert_eq!(fs::metadata(&blob).unwrap().len(), 7);
        // 已存在拒绝
        let err = f.root.link_file(&lp("docs/a.txt"), &blob).unwrap_err();
        assert!(matches!(err, ResolveError::AlreadyExists));
        // 父目录不存在 / target 不是文件
        let err = f.root.link_file(&lp("nope/b.txt"), &blob).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound));
        let err = f.root.link_file(&lp("c.txt"), &f.outside).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound));
    }

    #[test]
    fn remove_symlink_removes_link_not_target() {
        let f = fixture();
        fs::write(f.root_path.join("real.txt"), "x").unwrap();
        symlink(f.root_path.join("real.txt"), f.root_path.join("link")).unwrap();
        f.root.remove_entry(&lp("link"), false).unwrap();
        assert!(!f.root_path.join("link").exists());
        assert!(f.root_path.join("real.txt").exists());
    }
}
