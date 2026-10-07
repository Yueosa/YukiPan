//! 私有区路径规范化与目录树操作。
//!
//! - [`LogicalPath`]: 把外部输入规范化成相对逻辑路径, 建立不可逃逸的不变量;
//! - [`DataRoot`]: 数据根锚点, 把逻辑路径解析成保证落在根内的绝对路径,
//!   并承载目录树操作 (list / mkdir / move / delete, 见 tree 模块);
//! - [`DirEntry`]: 列目录返回的条目。
//!
//! 安全约定 (设计文档第 5、7 章):
//! - `..`、反斜杠、控制字符、NUL 一律拒绝, 不静默折叠;
//! - 解析会穿过符号链接校验真实位置, 指向根外即拒绝 (指向根内允许);
//! - 目录树操作不跟随符号链接 (链接条目不可见、删链接不碰目标);
//! - 错误信息不含真实磁盘路径。

mod path;
mod root;
mod tree;

pub use path::{LogicalPath, MAX_COMPONENT_LEN, MAX_PATH_LEN, PathError};
pub use root::{DataRoot, ResolveError};
pub use tree::DirEntry;
