#![cfg_attr(
    all(not(debug_assertions), feature = "native-ui"),
    windows_subsystem = "windows"
)]

use prismcast_core::{
    protocol::RUST_RPC_PORT,
    rpc::{generate_session_token, run_rpc_server, AppEngine, RpcServerConfig},
};
use std::io::Read;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    env_logger::init();

    let port = std::env::var("PRISMCAST_RPC_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|&p| p > 0)
        .unwrap_or(RUST_RPC_PORT);

    let token = std::env::var("PRISMCAST_RPC_TOKEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(generate_session_token);

    println!("====================================================");
    println!(
        " PrismCast Studio v{} — 100% Rust Native Studio",
        env!("CARGO_PKG_VERSION")
    );
    println!(" RPC / Event Bridge listening on 127.0.0.1:{}", port);
    println!("====================================================");

    let engine = AppEngine::new();
    let shutdown = CancellationToken::new();

    // Encerra graciosamente em Ctrl+C / SIGTERM ou quando o stdin do processo pai fecha (EOF).
    let shutdown_sig = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown_sig.cancel();
    });

    if std::env::var("PRISMCAST_WATCH_STDIN").as_deref() == Ok("1") {
        let shutdown_stdin = shutdown.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 64];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        shutdown_stdin.cancel();
                        break;
                    }
                    Ok(_) => {}
                }
            }
        });
    }

    #[cfg(feature = "native-ui")]
    let headless = std::env::var("PRISMCAST_HEADLESS").as_deref() == Ok("1")
        || std::env::args().any(|a| a == "--headless");

    #[cfg(feature = "native-ui")]
    if !headless {
        let eng_rpc = engine.clone();
        let shutdown_rpc = shutdown.clone();
        tokio::spawn(async move {
            let _ = run_rpc_server(
                eng_rpc,
                RpcServerConfig { port, token },
                shutdown_rpc,
            )
            .await;
        });

        let rt_handle = tokio::runtime::Handle::current();
        if let Err(err) = prismcast_core::native_ui::run_native_app(engine.clone(), rt_handle) {
            eprintln!("[prismcast-native-ui] {err}");
        }
        shutdown.cancel();
        engine.shutdown().await;
        return;
    }

    if let Err(err) = run_rpc_server(
        engine.clone(),
        RpcServerConfig { port, token },
        shutdown.clone(),
    )
    .await
    {
        eprintln!("[prismcast-rust] {err}");
    }

    engine.shutdown().await;
}
