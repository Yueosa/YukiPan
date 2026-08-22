//! 私有区路径规范化与目录树操作。
//!
//! 当前实现「路径」这一层:
//!
//! - [`LogicalPath`]: 把外部输入规范化成相对逻辑路径, 建立不可逃逸的不变量;
//! - [`DataRoot`]: 数据根锚点, 把逻辑路径解析成保证落在根内的绝对路径。
//!
//! 目录树操作 (list / mkdir / move / delete) 在后续切片加入。
//!
//! 安全约定 (设计文档第 5、7 章):
//! - `..`、反斜杠、控制字符、NUL 一律拒绝, 不静默折叠;
//! - 解析会穿过符号链接校验真实位置, 指向根外即拒绝 (指向根内允许);
//! - 错误信息不含真实磁盘路径。

mod path;
mod root;

pub use path::{LogicalPath, MAX_COMPONENT_LEN, MAX_PATH_LEN, PathError};
pub use root::{DataRoot, ResolveError};
