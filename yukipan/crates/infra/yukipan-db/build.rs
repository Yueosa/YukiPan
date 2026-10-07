//! sqlx::migrate! 在编译期把 migrations/*.sql 嵌入二进制, 但稳定版工具链下
//! 宏不会向 cargo 登记这些文件为依赖 (tracked::path 仅在 unstable 标志下启用)。
//! 没有这条指令时, 新增迁移文件不会触发本 crate 重编译, 测试会拿着旧迁移集合跑。

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
