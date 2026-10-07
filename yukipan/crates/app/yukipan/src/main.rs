//! YukiPan 服务端二进制: 入口、子命令分发与退出码。

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        // 默认即 serve; 服务生命周期归 systemd (systemctl), 不做 start/stop 子命令。
        [] | ["serve"] => yukipan_core::run().await,
        ["user", "add", username] => yukipan_core::user_add(username).await,
        ["thumbs", "rebuild"] => yukipan_core::thumbs_rebuild().await,
        _ => {
            eprintln!("用法:");
            eprintln!("  yukipan [serve]             运行 HTTP 服务 (默认, 由 systemd 拉起)");
            eprintln!("  yukipan user add <用户名>   交互式建用户");
            eprintln!("  yukipan thumbs rebuild      为存量图床图片补生成缩略图");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yukipan: {e}");
            ExitCode::FAILURE
        }
    }
}
