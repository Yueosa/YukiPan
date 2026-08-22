//! 逻辑路径: 私有区内相对于用户根的路径, 已规范化且不可逃逸。

use std::fmt;
use std::path::Path;

use thiserror::Error;

/// 单个路径分量允许的最大字节数 (常见文件系统上限 255)。
pub const MAX_COMPONENT_LEN: usize = 255;

/// 规范化后整条逻辑路径允许的最大字节数。
pub const MAX_PATH_LEN: usize = 1024;

/// 逻辑路径规范化/校验失败的原因。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PathError {
    /// 含 NUL 字节。
    #[error("路径包含 NUL 字节")]
    Nul,
    /// 含反斜杠 (避免 Windows 语义与转义歧义)。
    #[error("路径包含反斜杠 `\\`")]
    Backslash,
    /// 含控制字符。
    #[error("路径包含控制字符")]
    ControlChar,
    /// 含 `..` 分量。
    #[error("路径包含 `..` 分量, 不允许向上逃逸")]
    ParentEscape,
    /// 单个分量超过 [`MAX_COMPONENT_LEN`]。
    #[error("路径分量超过 {} 字节", MAX_COMPONENT_LEN)]
    ComponentTooLong,
    /// 整条路径超过 [`MAX_PATH_LEN`]。
    #[error("路径总长超过 {} 字节", MAX_PATH_LEN)]
    TooLong,
}

/// 规范化后的逻辑路径。
///
/// 不变量 (由 [`LogicalPath::parse`] 建立, 之后不可破坏):
/// - 相对路径, 根用空串表示;
/// - 分量以单个 `/` 分隔, 无空分量 / `.` / `..`;
/// - 不含 NUL、反斜杠、控制字符;
/// - 分量与总长均在限制内。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct LogicalPath(String);

impl LogicalPath {
    /// 解析并规范化外部输入。
    ///
    /// 空串、纯 `/` 视为根; 前导/尾部/重复 `/` 与 `.` 分量被折叠;
    /// `..` 一律拒绝 (不静默折叠, 避免掩盖穿越企图)。
    pub fn parse(input: &str) -> Result<Self, PathError> {
        if input.contains('\0') {
            return Err(PathError::Nul);
        }
        if input.contains('\\') {
            return Err(PathError::Backslash);
        }
        if input.chars().any(char::is_control) {
            return Err(PathError::ControlChar);
        }

        let mut out = String::with_capacity(input.len());
        for comp in input.split('/') {
            match comp {
                "" | "." => continue,
                ".." => return Err(PathError::ParentEscape),
                _ => {
                    if comp.len() > MAX_COMPONENT_LEN {
                        return Err(PathError::ComponentTooLong);
                    }
                    if !out.is_empty() {
                        out.push('/');
                    }
                    out.push_str(comp);
                }
            }
        }
        if out.len() > MAX_PATH_LEN {
            return Err(PathError::TooLong);
        }
        Ok(Self(out))
    }

    /// 根路径。
    pub fn root() -> Self {
        Self(String::new())
    }

    /// 规范化形态: 根为空串, 其余形如 `a/b/c`。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 是否为根。
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// 逐分量迭代 (根返回空迭代器)。
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|c| !c.is_empty())
    }

    /// 最后一个分量 (根无名字)。
    pub fn file_name(&self) -> Option<&str> {
        if self.is_root() {
            None
        } else {
            self.0.rsplit('/').next()
        }
    }

    /// 父路径 (根没有父; 顶层分量的父是根)。
    pub fn parent(&self) -> Option<LogicalPath> {
        if self.is_root() {
            return None;
        }
        match self.0.rsplit_once('/') {
            Some((head, _)) => Some(LogicalPath(head.to_string())),
            None => Some(LogicalPath::root()),
        }
    }

    /// 作为相对 [`Path`] 使用 (用于与数据根 join)。
    pub fn as_rel_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl fmt::Display for LogicalPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> String {
        LogicalPath::parse(input).unwrap().as_str().to_string()
    }

    #[test]
    fn root_forms() {
        assert_eq!(ok(""), "");
        assert_eq!(ok("/"), "");
        assert_eq!(ok("///"), "");
        assert_eq!(ok("/."), "");
        assert_eq!(ok("./."), "");
    }

    #[test]
    fn collapses_separators_and_dot() {
        assert_eq!(ok("a"), "a");
        assert_eq!(ok("/a/b/c"), "a/b/c");
        assert_eq!(ok("a//b///c"), "a/b/c");
        assert_eq!(ok("a/./b/."), "a/b");
        assert_eq!(ok("a/"), "a");
        assert_eq!(ok("./a"), "a");
    }

    #[test]
    fn keeps_legal_names() {
        assert_eq!(ok("中文/图片.png"), "中文/图片.png");
        assert_eq!(ok(".hidden/file"), ".hidden/file");
        assert_eq!(ok("a b/空 格.txt"), "a b/空 格.txt");
        assert_eq!(ok(".../..x"), ".../..x");
        // 全角反斜杠 U+FF3C 不是路径分隔符, 是合法文件名字符。
        assert_eq!(ok("全角＼名"), "全角＼名");
    }

    #[test]
    fn rejects_parent_escape() {
        for bad in ["..", "../a", "a/../b", "a/..", "../../etc", "a/../../b"] {
            assert_eq!(
                LogicalPath::parse(bad),
                Err(PathError::ParentEscape),
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_bad_bytes() {
        assert_eq!(LogicalPath::parse("a\0b"), Err(PathError::Nul));
        assert_eq!(LogicalPath::parse("a\\b"), Err(PathError::Backslash));
        assert_eq!(LogicalPath::parse("a\nb"), Err(PathError::ControlChar));
        assert_eq!(LogicalPath::parse("a\tb"), Err(PathError::ControlChar));
    }

    #[test]
    fn rejects_oversize() {
        let comp = "x".repeat(MAX_COMPONENT_LEN + 1);
        assert_eq!(LogicalPath::parse(&comp), Err(PathError::ComponentTooLong));

        let mut long = String::new();
        while long.len() <= MAX_PATH_LEN {
            long.push_str("aa/");
        }
        assert_eq!(LogicalPath::parse(&long), Err(PathError::TooLong));
    }

    #[test]
    fn boundary_component_len_passes() {
        let comp = "x".repeat(MAX_COMPONENT_LEN);
        assert!(LogicalPath::parse(&comp).is_ok());
    }

    #[test]
    fn file_name_and_parent() {
        let p = LogicalPath::parse("a/b/c.txt").unwrap();
        assert_eq!(p.file_name(), Some("c.txt"));
        assert_eq!(p.parent().unwrap().as_str(), "a/b");

        let top = LogicalPath::parse("a").unwrap();
        assert_eq!(top.file_name(), Some("a"));
        assert_eq!(top.parent().unwrap().as_str(), "");

        let root = LogicalPath::root();
        assert_eq!(root.file_name(), None);
        assert_eq!(root.parent(), None);
    }

    #[test]
    fn components_iter() {
        let p = LogicalPath::parse("a/b/c").unwrap();
        assert_eq!(p.components().collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(LogicalPath::root().components().count(), 0);
    }

    #[test]
    fn display_roundtrip() {
        let p = LogicalPath::parse("/a//b/").unwrap();
        assert_eq!(p.to_string(), "a/b");
        assert!(LogicalPath::root().is_root());
        assert!(LogicalPath::default().is_root());
    }
}
