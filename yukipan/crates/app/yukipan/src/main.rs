//! YukiPan 服务端二进制: 入口、运行时与退出码。

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match yukipan_core::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yukipan: {e}");
            ExitCode::FAILURE
        }
    }
}
