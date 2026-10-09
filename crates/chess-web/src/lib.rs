//! # chess-web
//!
//! HTTP layer on top of [`chess_hub`]: serves the web app (a single gzipped file)
//! and forwards `/api/*` requests to the hub. The same code runs on the board
//! (picoserve + embassy-net) and in the PC simulator (picoserve + tokio).
//!
//! Differences between the board and the PC are hidden behind the [`Platform`]
//! trait (time, hub lock, WiFi).

#![no_std]

use core::fmt::{self, Write};

use chess_hub::{ApiOutcome, Hub};
use picoserve::ResponseSent;
use picoserve::io::Read;
use picoserve::request::{Path, Request};
use picoserve::response::{IntoResponse, Response, ResponseWriter, StatusCode};
use picoserve::routing::PathRouterService;

/// The web app (HTML + CSS + JS + pieces), gzipped.
pub static INDEX_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index.html.gz"));

/// JSON response buffer size (lobby / game fit with room to spare).
pub const JSON_BUFFER: usize = 5120;

/// What the environment (board or simulator) must provide.
pub trait Platform {
    /// Monotonic time in milliseconds.
    fn now_ms(&self) -> u64;

    /// Runs `f` with the hub locked (critical section on the board, mutex on the PC).
    fn with_hub<R>(&self, f: impl FnOnce(&mut Hub) -> R) -> R;

    /// Stores WiFi credentials and starts connecting.
    fn wifi_save(&self, ssid: &str, password: &str);

    /// Forgets the WiFi network (back to AP-only mode).
    fn wifi_forget(&self);

    /// Writes `{"networks":[{"ssid":"…","rssi":-50,"secure":true},…]}` –
    /// the last known scan result – and starts a new scan if needed.
    fn wifi_scan(&self, out: &mut dyn Write) -> fmt::Result;

    /// The board's address in its own AP (for the captive portal redirect).
    fn portal_host(&self) -> &str;
}

/// Paths that phones and computers probe to detect whether the network is
/// behind a login page. We answer with a redirect, so the OS opens our app
/// (captive portal).
const CAPTIVE_PROBES: [&str; 9] = [
    "/generate_204",
    "/gen_204",
    "/hotspot-detect.html",
    "/library/test/success.html",
    "/connecttest.txt",
    "/ncsi.txt",
    "/redirect",
    "/canonical.html",
    "/success.txt",
];

/// Response kind by path.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    App,
    Api,
    WifiScan,
    CaptiveProbe,
    NotFound,
}

/// Decides where a request belongs.
pub fn route(path: &str) -> Route {
    match path {
        "/" | "/index.html" => Route::App,
        "/api/wifi/scan" => Route::WifiScan,
        p if p.starts_with("/api/") => Route::Api,
        p if CAPTIVE_PROBES.contains(&p) => Route::CaptiveProbe,
        _ => Route::NotFound,
    }
}

/// Handles an API request: locks the hub, writes JSON into `out` and performs
/// a WiFi action if requested. Returns `false` if the path does not exist.
pub fn handle_api<P: Platform>(
    platform: &P,
    path: &str,
    query: &str,
    out: &mut heapless::String<JSON_BUFFER>,
) -> bool {
    let now = platform.now_ms();
    let outcome = platform.with_hub(|hub| chess_hub::handle(hub, now, path, query, out));
    match outcome {
        Ok(ApiOutcome::Json) => true,
        Ok(ApiOutcome::NotFound) => false,
        Ok(ApiOutcome::WifiSave { ssid, password }) => {
            platform.wifi_save(&ssid, &password);
            true
        }
        Ok(ApiOutcome::WifiForget) => {
            platform.wifi_forget();
            true
        }
        Err(_) => {
            // The response did not fit into the buffer – send a clear error.
            out.clear();
            let _ = out.push_str("{\"ok\":false,\"err\":\"overflow\"}");
            true
        }
    }
}

/// WiFi scan for the owner only (otherwise anyone could see nearby networks).
pub fn handle_wifi_scan<P: Platform>(
    platform: &P,
    query: &str,
    out: &mut heapless::String<JSON_BUFFER>,
) {
    let owner =
        chess_hub::params::token(query).is_some_and(|t| platform.with_hub(|hub| hub.is_owner(t)));
    if !owner {
        let _ = out.push_str("{\"ok\":false,\"err\":\"not_allowed\",\"networks\":[]}");
        return;
    }
    if platform.wifi_scan(out).is_err() {
        out.clear();
        let _ = out.push_str("{\"networks\":[]}");
    }
}

/// URL for the `Location` header (allocation-free Display).
struct PortalUrl<'a>(&'a str);

impl fmt::Display for PortalUrl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "http://{}/", self.0)
    }
}

/// picoserve service: every request lands here (custom routing via [`route`]).
pub struct ChessService<P: 'static> {
    pub platform: &'static P,
}

impl<P: Platform> PathRouterService<()> for ChessService<P> {
    async fn call_path_router_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        _state: &(),
        _parameters: (),
        path: Path<'_>,
        request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let path = path.encoded();
        let query = request.parts.query().map_or("", |q| q.0);
        let connection = request.body_connection.finalize().await?;

        match route(path) {
            Route::App => {
                Response::ok(INDEX_HTML_GZ)
                    .with_content_type("text/html; charset=utf-8")
                    .with_header("Content-Encoding", "gzip")
                    .with_header("Cache-Control", "no-cache")
                    .write_to(connection, response_writer)
                    .await
            }
            Route::Api | Route::WifiScan => {
                let mut json = heapless::String::<JSON_BUFFER>::new();
                let found = if route(path) == Route::WifiScan {
                    handle_wifi_scan(self.platform, query, &mut json);
                    true
                } else {
                    handle_api(self.platform, path, query, &mut json)
                };
                if found {
                    Response::ok(json.as_str())
                        .with_content_type("application/json; charset=utf-8")
                        .with_header("Cache-Control", "no-store")
                        .write_to(connection, response_writer)
                        .await
                } else {
                    Response::new(
                        StatusCode::NOT_FOUND,
                        "{\"ok\":false,\"err\":\"not_found\"}",
                    )
                    .with_content_type("application/json; charset=utf-8")
                    .write_to(connection, response_writer)
                    .await
                }
            }
            Route::CaptiveProbe => {
                Response::new(StatusCode::FOUND, "")
                    .with_header("Location", PortalUrl(self.platform.portal_host()))
                    .with_header("Cache-Control", "no-store")
                    .write_to(connection, response_writer)
                    .await
            }
            Route::NotFound => {
                Response::new(StatusCode::NOT_FOUND, "Not found")
                    .write_to(connection, response_writer)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing() {
        assert_eq!(route("/"), Route::App);
        assert_eq!(route("/api/lobby"), Route::Api);
        assert_eq!(route("/api/wifi/scan"), Route::WifiScan);
        assert_eq!(route("/generate_204"), Route::CaptiveProbe);
        assert_eq!(route("/hotspot-detect.html"), Route::CaptiveProbe);
        assert_eq!(route("/favicon.ico"), Route::NotFound);
    }

    #[test]
    fn app_is_gzipped_and_small() {
        // gzip header 1F 8B.
        assert_eq!(&INDEX_HTML_GZ[..2], &[0x1f, 0x8b]);
        assert!(
            INDEX_HTML_GZ.len() < 80_000,
            "app is {} B",
            INDEX_HTML_GZ.len()
        );
    }
}
