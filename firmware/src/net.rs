//! Networking: own Wi-Fi (AP) + connection to the home network (STA), DHCP server,
//! captive DNS and the web server.
//!
//! ```text
//! AP  192.168.4.1/24 ── DHCP server (edge-dhcp) ── DNS "everything → 192.168.4.1" (edge-captive)
//!                    └─ HTTP :80 (picoserve) × WEB_TASKS_AP
//! STA DHCP client    └─ HTTP :80 (picoserve) × WEB_TASKS_STA
//! ```
//! The Wi-Fi chip always runs in AP+STA mode: the AP is always available (fallback, first
//! setup), the STA connects to the saved home network.

use core::fmt::Write as _;
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use chess_hub::stored::WifiCredentials;
use chess_web::ChessService;
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_net::{Config, Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{
    AuthenticationMethod, AuthenticationMethodConfig, Config as WifiConfig, Interface,
    WifiController, ap::AccessPointConfig, scan::ScanConfig, sta::StationConfig,
};
use log::{info, warn};
use picoserve::routing::ServicePathRouter;
use static_cell::StaticCell;

use crate::shared::{
    self, AP_HOST, AP_IP, PLATFORM, SCAN_RESULTS, ScanItem, WIFI_COMMANDS, WifiCommand, with_hub,
};
use crate::stats;
use crate::storage;

/// Number of web tasks (concurrent HTTP connections) on the AP and on the STA.
/// (Most players come via the home network – it used to have only 3 tasks, and with all of
/// them busy the board refused new connections.)
const WEB_TASKS_AP: usize = crate::chip::WEB_TASKS_AP;
const WEB_TASKS_STA: usize = crate::chip::WEB_TASKS_STA;
/// Total number of web tasks (for the statistics on the display).
pub const WEB_TASKS: usize = WEB_TASKS_AP + WEB_TASKS_STA;
/// How long to wait for the home Wi-Fi connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Retry delay after a failed connection.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// Maximum Wi-Fi TX power in units of 0.25 dBm (60 = 15 dBm).
/// esp-radio sets only 20 (5 dBm) at startup – too little for a board with a small antenna.
/// According to esp-radio, values above ~65 (16 dBm) cause authentication failures on some
/// hardware.
const WIFI_TX_POWER: i8 = 60;

/// Actual maximum TX power according to the Wi-Fi library (units of 0.25 dBm), `None` when
/// Wi-Fi is not running. esp-radio does not expose this function, but it is in the linked Wi-Fi
/// library.
pub fn tx_power_quarter_dbm() -> Option<i8> {
    unsafe extern "C" {
        fn esp_wifi_get_max_tx_power(power: *mut i8) -> i32;
    }
    let mut power = 0i8;
    // SAFETY: ESP-IDF Wi-Fi function (libnet80211), writes only to `power`; 0 = ESP_OK.
    (unsafe { esp_wifi_get_max_tx_power(&mut power) } == 0).then_some(power)
}

/// Sets the TX power (after every `set_config`, which may restart Wi-Fi).
fn apply_tx_power(controller: &mut WifiController<'static>) {
    match controller.set_max_tx_power(WIFI_TX_POWER) {
        Ok(()) => info!("Wi-Fi: max TX power {} dBm", WIFI_TX_POWER as f32 / 4.0),
        Err(error) => warn!("Wi-Fi: setting the TX power failed: {error:?}"),
    }
}

type AppRouter = picoserve::Router<ServicePathRouter<ChessService<shared::BoardPlatform>>>;

// Sockets: web tasks + DHCP server + captive DNS (AP), web tasks + DHCP client (STA).
static AP_RESOURCES: StaticCell<StackResources<{ WEB_TASKS_AP + 3 }>> = StaticCell::new();
static STA_RESOURCES: StaticCell<StackResources<{ WEB_TASKS_STA + 2 }>> = StaticCell::new();
static APP: StaticCell<AppRouter> = StaticCell::new();
/// The connection is closed after every response (no keep-alive): the web only does short
/// polling, and idle keep-alive connections (including those a browser opens "in advance")
/// would otherwise block the limited number of web tasks and other clients would wait.
static WEB_CONFIG: picoserve::Config = picoserve::Config::new(picoserve::Timeouts {
    // browsers open connections in advance and may send nothing on them – do not block a task for 5
    // s
    start_read_request: Duration::from_secs(2),
    persistent_start_read_request: Duration::from_secs(1),
    // Also the limit for how long to wait after the response for the client to close the
    // connection (picoserve shutdown): a phone in power-saving mode does that late and the task
    // would needlessly hang for up to 3 s. By then the client has the whole response; requests
    // are short (1 packet).
    read_request: Duration::from_millis(800),
    write: Duration::from_secs(3),
})
.close_connection_after_response();

/// Name of the own network: "ESP-Chess-XXXX" from the end of the MAC address.
pub fn ap_ssid(mac: [u8; 6]) -> heapless::String<32> {
    let mut name = heapless::String::new();
    let _ = write!(name, "ESP-Chess-{:02X}{:02X}", mac[4], mac[5]);
    name
}

/// Initialises Wi-Fi and spawns all network tasks.
pub fn start(
    spawner: Spawner,
    wifi: WIFI<'static>,
    seed: u64,
    credentials: Option<WifiCredentials>,
) {
    let controller =
        WifiController::new(wifi, Default::default()).expect("Wi-Fi initialisation failed");
    let ap_interface = Interface::access_point();
    let sta_interface = Interface::station();
    let ssid = ap_ssid(ap_interface.mac_address());
    info!("Own Wi-Fi: {ssid} (http://{AP_HOST}/)");
    with_hub(|hub| {
        hub.net.ap_ssid.clear();
        let _ = hub.net.ap_ssid.push_str(&ssid);
        hub.net.mode.clear();
        let _ = hub.net.mode.push_str("ap");
    });

    // --- Network stacks (embassy-net) ---
    let ap_ip = Ipv4Address::new(AP_IP[0], AP_IP[1], AP_IP[2], AP_IP[3]);
    let ap_config = Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(ap_ip, 24),
        gateway: Some(ap_ip),
        dns_servers: Default::default(),
    });
    let (ap_stack, ap_runner) = embassy_net::new(
        ap_interface,
        ap_config,
        AP_RESOURCES.init(StackResources::new()),
        seed,
    );
    let (sta_stack, sta_runner) = embassy_net::new(
        sta_interface,
        Config::dhcpv4(Default::default()),
        STA_RESOURCES.init(StackResources::new()),
        seed ^ 0x5A5A_5A5A,
    );

    let app = APP.init(picoserve::Router::from_service(ChessService {
        platform: &PLATFORM,
    }));

    spawner.spawn(net_runner(ap_runner).unwrap());
    spawner.spawn(net_runner(sta_runner).unwrap());
    spawner.spawn(dhcp_server(ap_stack).unwrap());
    spawner.spawn(captive_dns(ap_stack).unwrap());
    for id in 0..WEB_TASKS_AP {
        spawner.spawn(web_task(id, ap_stack, app).unwrap());
    }
    for id in 0..WEB_TASKS_STA {
        spawner.spawn(web_task(WEB_TASKS_AP + id, sta_stack, app).unwrap());
    }
    spawner.spawn(wifi_manager(controller, sta_stack, ssid, credentials).unwrap());
}

/// Runs a network stack (packets between the Wi-Fi chip and embassy-net).
#[embassy_executor::task(pool_size = 2)]
async fn net_runner(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

/// Web server – each task serves one connection at a time (with statistics, see `stats`).
#[embassy_executor::task(pool_size = WEB_TASKS_AP + WEB_TASKS_STA)]
async fn web_task(id: usize, stack: Stack<'static>, app: &'static AppRouter) -> ! {
    // Requests are short GET/POSTs (token in the URL, no cookies); responses are sent in chunks.
    let mut rx = [0u8; 512];
    let mut tx = [0u8; 1024];
    let mut http = [0u8; 1280];
    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx, &mut tx);
        if let Err(error) = socket.accept(80).await {
            warn!("web {id}: accept {error:?}");
            continue;
        }
        // Safety net at the TCP level; HTTP timeouts are enforced by picoserve (WEB_CONFIG).
        socket.set_timeout(Some(Duration::from_secs(10)));
        let start = stats::connection_start();
        let result = picoserve::Server::new(app, &WEB_CONFIG, &mut http)
            .serve(socket)
            .await;
        let (requests, error) = match result {
            Ok(info) => (
                info.handled_requests_count.min(u32::MAX as u64) as u32,
                false,
            ),
            Err(_) => (0, true),
        };
        stats::connection_end(start, requests, error);
    }
}

/// DHCP server for the own Wi-Fi: hands out 192.168.4.50–200 and announces the board as the DNS
/// server and the captive portal URL.
#[embassy_executor::task]
async fn dhcp_server(stack: Stack<'static>) -> ! {
    use edge_nal::UdpBind;

    let ip = Ipv4Addr::new(AP_IP[0], AP_IP[1], AP_IP[2], AP_IP[3]);
    let buffers = edge_nal_embassy::UdpBuffers::<1, 1024, 1024, 4>::new();
    let udp = edge_nal_embassy::Udp::new(stack, &buffers);
    let mut buf = [0u8; 1500];
    let mut gateway = [ip];
    let dns = [ip];
    let mut options = edge_dhcp::server::ServerOptions::new(ip, Some(&mut gateway));
    options.dns = &dns;
    options.captive_url = Some("http://192.168.4.1/");
    let mut server =
        edge_dhcp::server::Server::<_, 16>::new(|| embassy_time::Instant::now().as_secs(), ip);

    loop {
        let bind = udp
            .bind(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                edge_dhcp::io::DEFAULT_SERVER_PORT,
            )))
            .await;
        match bind {
            Ok(mut socket) => {
                if let Err(error) =
                    edge_dhcp::io::server::run(&mut server, &options, &mut socket, &mut buf).await
                {
                    warn!("DHCP server: {error:?}");
                }
            }
            Err(error) => warn!("DHCP bind: {error:?}"),
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// DNS server that answers every query with the board's address → after joining the AP a
/// phone opens the sign-in page (captive portal) = our app.
#[embassy_executor::task]
async fn captive_dns(stack: Stack<'static>) -> ! {
    let ip = Ipv4Addr::new(AP_IP[0], AP_IP[1], AP_IP[2], AP_IP[3]);
    let buffers = edge_nal_embassy::UdpBuffers::<1, 512, 512, 4>::new();
    let udp = edge_nal_embassy::Udp::new(stack, &buffers);
    let mut tx = [0u8; 512];
    let mut rx = [0u8; 512];
    loop {
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 53));
        if let Err(error) = edge_captive::io::run(
            &udp,
            address,
            &mut tx,
            &mut rx,
            ip,
            core::time::Duration::from_secs(60),
        )
        .await
        {
            warn!("Captive DNS: {error:?}");
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// Wi-Fi chip configuration: always the AP, the STA from the saved credentials.
fn wifi_config(ap_ssid: &str, credentials: Option<&WifiCredentials>) -> WifiConfig {
    let ap = AccessPointConfig::default()
        .with_ssid(ap_ssid.try_into().expect("valid SSID"))
        .with_authentication(AuthenticationMethodConfig::Open)
        .with_max_connections(10);
    let mut sta = StationConfig::default();
    if let Some(credentials) = credentials {
        let authentication = if credentials.password.is_empty() {
            AuthenticationMethodConfig::Open
        } else {
            AuthenticationMethodConfig::Wpa2Personal(
                credentials.password.as_str().try_into().unwrap_or_default(),
            )
        };
        sta = sta
            .with_ssid(credentials.ssid.as_str().try_into().unwrap_or_default())
            .with_authentication(authentication);
    }
    WifiConfig::AccessPointStation(sta, ap)
}

fn set_net_status(connecting: bool, ssid: &str, ip: Option<Ipv4Address>, error: &str) {
    with_hub(|hub| {
        let net = &mut hub.net;
        net.connecting = connecting;
        net.sta_ssid.clear();
        let _ = net.sta_ssid.push_str(ssid);
        net.sta_ip.clear();
        if let Some(ip) = ip {
            let _ = write!(net.sta_ip, "{ip}");
        }
        net.sta_error.clear();
        let _ = net.sta_error.push_str(error);
        net.mode.clear();
        let _ = net.mode.push_str(if ip.is_some() { "sta" } else { "ap" });
    });
}

/// Tries to connect to the home Wi-Fi; returns `true` on success (including an IP address).
async fn connect(
    controller: &mut WifiController<'static>,
    stack: Stack<'static>,
    credentials: &WifiCredentials,
) -> bool {
    info!("Connecting to Wi-Fi \"{}\"…", credentials.ssid);
    set_net_status(true, &credentials.ssid, None, "");
    match with_timeout(CONNECT_TIMEOUT, controller.connect_async()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            warn!("Connection failed: {error:?}");
            set_net_status(false, &credentials.ssid, None, "failed");
            return false;
        }
        Err(_) => {
            warn!("Connection timed out");
            set_net_status(false, &credentials.ssid, None, "timeout");
            return false;
        }
    }
    // Wait for an address from DHCP.
    if with_timeout(CONNECT_TIMEOUT, stack.wait_config_up())
        .await
        .is_err()
    {
        warn!("No IP address received");
        set_net_status(false, &credentials.ssid, None, "dhcp");
        return false;
    }
    let ip = stack.config_v4().map(|c| c.address.address());
    info!("Connected, address http://{:?}/", ip);
    set_net_status(false, &credentials.ssid, ip, "");
    true
}

/// Wi-Fi manager: connecting, retries, commands from the web (save, forget, scan).
#[embassy_executor::task]
async fn wifi_manager(
    mut controller: WifiController<'static>,
    sta_stack: Stack<'static>,
    ap_ssid: heapless::String<32>,
    mut credentials: Option<WifiCredentials>,
) -> ! {
    if let Err(error) = controller.set_config(&wifi_config(&ap_ssid, credentials.as_ref())) {
        warn!("Wi-Fi configuration failed: {error:?}");
    }
    apply_tx_power(&mut controller);
    let mut connected = false;
    if let Some(creds) = &credentials {
        connected = connect(&mut controller, sta_stack, creds).await;
    }

    loop {
        // Wait for a command, a disconnect, or the time of the next connection attempt.
        // (The block makes sure the waiting futures are dropped before `controller` is used again.)
        let event = {
            let retry = async {
                if credentials.is_some() && !connected {
                    Timer::after(RETRY_AFTER).await
                } else {
                    core::future::pending::<()>().await
                }
            };
            let disconnect = async {
                if connected {
                    let _ = controller.wait_for_disconnect_async().await;
                } else {
                    core::future::pending::<()>().await
                }
            };
            select3(WIFI_COMMANDS.receive(), disconnect, retry).await
        };
        match event {
            Either3::First(WifiCommand::Save(new)) => {
                storage::save(&new);
                if connected {
                    let _ = controller.disconnect_async().await;
                }
                credentials = Some(new);
                if let Err(error) =
                    controller.set_config(&wifi_config(&ap_ssid, credentials.as_ref()))
                {
                    warn!("Wi-Fi configuration failed: {error:?}");
                }
                apply_tx_power(&mut controller);
                connected =
                    connect(&mut controller, sta_stack, credentials.as_ref().unwrap()).await;
            }
            Either3::First(WifiCommand::Forget) => {
                storage::erase();
                if connected {
                    let _ = controller.disconnect_async().await;
                }
                connected = false;
                credentials = None;
                let _ = controller.set_config(&wifi_config(&ap_ssid, None));
                apply_tx_power(&mut controller);
                set_net_status(false, "", None, "");
            }
            Either3::First(WifiCommand::Scan) => scan(&mut controller).await,
            Either3::Second(()) => {
                warn!("Wi-Fi disconnected, will try to reconnect");
                connected = false;
                set_net_status(
                    false,
                    credentials.as_ref().map_or("", |c| c.ssid.as_str()),
                    None,
                    "lost",
                );
            }
            Either3::Third(()) => {
                if let Some(creds) = &credentials {
                    connected = connect(&mut controller, sta_stack, creds).await;
                }
            }
        }
    }
}

/// Scans nearby networks → `SCAN_RESULTS` (the web picks them up with its next request).
async fn scan(controller: &mut WifiController<'static>) {
    let config = ScanConfig::default().with_max(16);
    match controller.scan_async(&config).await {
        Ok(found) => {
            SCAN_RESULTS.lock(|cell| {
                let mut results = cell.borrow_mut();
                results.clear();
                for ap in found.iter() {
                    if ap.ssid.is_empty()
                        || results.iter().any(|r| r.ssid.as_str() == ap.ssid.as_str())
                    {
                        continue; // skip hidden networks and duplicates (mesh)
                    }
                    let mut ssid = heapless::String::new();
                    let _ = ssid.push_str(ap.ssid.as_str());
                    let item = ScanItem {
                        ssid,
                        rssi: ap.signal_strength,
                        secure: !matches!(ap.auth_method, Some(AuthenticationMethod::None) | None),
                    };
                    if results.push(item).is_err() {
                        break;
                    }
                }
                results.sort_unstable_by_key(|a| core::cmp::Reverse(a.rssi));
            });
        }
        Err(error) => warn!("Wi-Fi scan failed: {error:?}"),
    }
}
