//! Board simulator for the PC.
//!
//! Runs the **same** web server (chess-web), game logic (chess-hub) and
//! engine (chess-engine) as the firmware – it just serves on localhost instead
//! of using WiFi and a display. Used for web development and Playwright tests.
//!
//! ```text
//! chess-sim [--port 8080] [--fast] [--no-demo] [--no-provision]
//! chess-sim --uci          # engine as a UCI program (Arena, cutechess, …)
//! ```

mod engine_worker;
mod platform;
mod uci;

use std::rc::Rc;
use std::time::Duration;

use chess_web::ChessService;
use platform::SimPlatform;

/// Command-line options.
pub struct Options {
    pub port: u16,
    /// Faster engine (move time / 20) – for tests.
    pub fast: bool,
    pub demo: bool,
    /// Open provisioning – the first user to join may set up WiFi (as after a board reset).
    pub provision: bool,
}

fn parse_args() -> Result<(Options, bool), String> {
    let mut options = Options {
        port: 8080,
        fast: false,
        demo: true,
        provision: true,
    };
    let mut uci = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                options.port = args
                    .next()
                    .and_then(|p| p.parse().ok())
                    .ok_or("--port requires a number")?
            }
            "--fast" => options.fast = true,
            "--no-demo" => options.demo = false,
            "--no-provision" => options.provision = false,
            "--uci" => uci = true,
            "--help" | "-h" => {
                println!("chess-sim [--port 8080] [--fast] [--no-demo] [--no-provision] | --uci");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok((options, uci))
}

fn main() {
    let (options, uci_mode) = match parse_args() {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    if uci_mode {
        uci::run();
        return;
    }

    // The platform lives for the whole program run (&'static for the picoserve service).
    let platform: &'static SimPlatform = Box::leak(Box::new(SimPlatform::new(&options)));
    engine_worker::spawn(platform, options.fast);
    platform::spawn_ticker(platform);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(serve(platform, options.port));
}

async fn serve(platform: &'static SimPlatform, port: u16) {
    let app = Rc::new(picoserve::Router::from_service(ChessService { platform }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|e| panic!("port {port} is in use: {e}"));
    println!("ESP32 Chess simulator: http://localhost:{port}/");

    static CONFIG: picoserve::Config = picoserve::Config::const_default().keep_connection_alive();

    tokio::task::LocalSet::new()
        .run_until(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let app = app.clone();
                tokio::task::spawn_local(async move {
                    let mut buffer = [0u8; 2048];
                    let _ = picoserve::Server::new_tokio(&app, &CONFIG, &mut buffer)
                        .serve(stream)
                        .await;
                });
            }
        })
        .await;
}
