//! 可执行入口。真正的模块树与启动流程都在 `lib.rs`，这里只负责把错误打出来
//! 并给出退出码。
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = miyu::run().await {
        eprintln!("{}: {error:#}", miyu::error_label());
        std::process::exit(1);
    }
}
