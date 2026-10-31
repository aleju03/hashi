use mania_bridge::{start, Options};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(mania_bridge::lazer::MODE) {
        std::process::exit(mania_bridge::lazer::writer_main(&args[1..]));
    }
    let bridge = match start(Options::default()).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate() => {}
    }
    bridge.shutdown().await;
    std::process::exit(0);
}

#[cfg(unix)]
async fn terminate() {
    if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        s.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(not(unix))]
async fn terminate() {
    std::future::pending::<()>().await;
}
