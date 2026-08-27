use slipstream_server::{Config, start_server};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let config = match Config::from_process_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Slipstream startup failed: {error}");
            std::process::exit(1);
        }
    };
    let server = match start_server(config).await {
        Ok(server) => server,
        Err(error) => {
            eprintln!("Slipstream startup failed: {error}");
            std::process::exit(1);
        }
    };
    println!("Slipstream listening at {}", server.url);
    wait_for_shutdown_signal().await;
    if let Err(error) = server.close().await {
        eprintln!("Slipstream shutdown failed: {error}");
        std::process::exit(1);
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
