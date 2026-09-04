use slipstream_server::{Config, ExpansionConfig, expand_library, start_server};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let command = arguments.next();
    if arguments.next().is_some() {
        eprintln!("Slipstream startup failed: unexpected command arguments");
        std::process::exit(1);
    }

    if let Some(command) = command {
        if command != "expand-library" {
            eprintln!("Slipstream startup failed: unknown command");
            std::process::exit(1);
        }
        let config = match ExpansionConfig::from_process_environment() {
            Ok(config) => config,
            Err(error) => {
                eprintln!("Slipstream Library expansion failed: {error}");
                std::process::exit(1);
            }
        };
        if let Err(error) = expand_library(config).await {
            eprintln!("Slipstream Library expansion failed: {error}");
            std::process::exit(1);
        }
        println!("Slipstream Library expansion completed");
        return;
    }

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
