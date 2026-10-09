//! Display texts (English by default, Czech optional).

/// Display language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Cs,
}

/// Text keys.
#[derive(Clone, Copy, Debug)]
pub enum Key {
    Starting,
    Players,
    Games,
    Depth,
    Scan,
    JoinWifi,
    OpenBrowser,
    ResetIn,
    ReleaseToCancel,
    Resetting,
    NoGames,
    Demo,
}

impl Lang {
    pub fn text(self, key: Key) -> &'static str {
        match (self, key) {
            (Lang::En, Key::Starting) => "Starting...",
            (Lang::Cs, Key::Starting) => "Startuji...",
            (Lang::En, Key::Players) => "Players",
            (Lang::Cs, Key::Players) => "Hráči",
            (Lang::En, Key::Games) => "Games",
            (Lang::Cs, Key::Games) => "Partie",
            (Lang::En, Key::Depth) => "D",
            (Lang::Cs, Key::Depth) => "H",
            (Lang::En, Key::Scan) => "Scan to play",
            (Lang::Cs, Key::Scan) => "Naskenuj a hraj",
            (Lang::En, Key::JoinWifi) => "1) Join Wi-Fi",
            (Lang::Cs, Key::JoinWifi) => "1) Připoj se k Wi-Fi",
            (Lang::En, Key::OpenBrowser) => "2) Open in browser",
            (Lang::Cs, Key::OpenBrowser) => "2) Otevři v prohlížeči",
            (Lang::En, Key::ResetIn) => "Factory reset in",
            (Lang::Cs, Key::ResetIn) => "Tovární reset za",
            (Lang::En, Key::ReleaseToCancel) => "Release BOOT to cancel",
            (Lang::Cs, Key::ReleaseToCancel) => "Pusť BOOT pro zrušení",
            (Lang::En, Key::Resetting) => "Wi-Fi erased, restarting...",
            (Lang::Cs, Key::Resetting) => "Wi-Fi smazána, restartuji...",
            (Lang::En, Key::NoGames) => "No games - open the web!",
            (Lang::Cs, Key::NoGames) => "Nic se nehraje - otevři web!",
            (Lang::En, Key::Demo) => "Demo",
            (Lang::Cs, Key::Demo) => "Ukázka",
        }
    }
}
