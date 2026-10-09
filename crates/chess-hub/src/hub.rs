//! `Hub` – the complete game server state: players, seeks, games and the engine queue.
//!
//! The hub knows nothing about the network or hardware. It receives commands
//! (log in a player, play a move, …) with the current time in ms and returns
//! results. This makes it fully testable on a PC, and the same code runs on
//! the board and in the development simulator.

use chess_core::{Color, Move, Position, parse_uci};
use chess_engine::{PV_LEN, SearchLimits, level_limits};
use heapless::Vec;

use crate::game::{EndReason, Game, GameId, MoveError, Outcome, Participant, TimeControl};
use crate::player::{Nick, NickError, Player, PlayerId, same_nick, validate_nick};

/// Maximum number of players (sessions) the hub keeps at once.
pub const MAX_PLAYERS: usize = 24;
/// Maximum number of games at once (including finished ones and the demo).
pub const MAX_GAMES: usize = 8;
/// Maximum number of open seeks.
pub const MAX_SEEKS: usize = 8;

/// A player is online if seen within the last 20 s (the web app polls every ~1 s).
pub const ONLINE_WINDOW_MS: u64 = 20_000;
/// Inactivity after which a session without an active game is dropped.
pub const SESSION_TIMEOUT_MS: u64 = 10 * 60_000;
/// Absence after which an active game ends as abandoned.
pub const ABANDON_MS: u64 = 3 * 60_000;
/// How long a finished game stays available for viewing.
pub const FINISHED_KEEP_MS: u64 = 10 * 60_000;
/// Time per move in the demo game.
pub const DEMO_MOVE_MS: u64 = 10_000;
/// Hint: search time, but always at least to depth `HINT_MIN_DEPTH` (hard cap
/// `HINT_HARD_MS` only for unusually complex positions).
pub const HINT_MS: u64 = 1_000;
pub const HINT_MIN_DEPTH: u8 = 8;
pub const HINT_HARD_MS: u64 = 15_000;
/// Pause between demo games.
pub const DEMO_RESTART_MS: u64 = 15_000;
/// While people are online the demo has the lowest priority: shorter move time…
pub const DEMO_MOVE_BUSY_MS: u64 = 2_000;
/// …and a pause between moves so the engine doesn't steal CPU or flash cache from networking.
pub const DEMO_PAUSE_BUSY_MS: u64 = 8_000;
/// Pause between demo moves while nobody is online: 10 s thinking, 20 s idle (~⅓ load,
/// the chip runs cooler). The first move of a new game has no pause.
pub const DEMO_PAUSE_IDLE_MS: u64 = 20_000;

/// Errors returned to the web app (the code is translated in the browser).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubError {
    BadNick,
    NickTaken,
    Full,
    NoSession,
    NoGame,
    NoSeek,
    NotPlayer,
    AlreadyPlaying,
    NotYourTurn,
    Illegal,
    GameOver,
    NotAllowed,
    BadRequest,
}

impl HubError {
    pub fn code(self) -> &'static str {
        match self {
            HubError::BadNick => "bad_nick",
            HubError::NickTaken => "nick_taken",
            HubError::Full => "full",
            HubError::NoSession => "no_session",
            HubError::NoGame => "no_game",
            HubError::NoSeek => "no_seek",
            HubError::NotPlayer => "not_player",
            HubError::AlreadyPlaying => "already_playing",
            HubError::NotYourTurn => "not_your_turn",
            HubError::Illegal => "illegal",
            HubError::GameOver => "game_over",
            HubError::NotAllowed => "not_allowed",
            HubError::BadRequest => "bad_request",
        }
    }
}

impl From<NickError> for HubError {
    fn from(_: NickError) -> Self {
        HubError::BadNick
    }
}

impl From<MoveError> for HubError {
    fn from(error: MoveError) -> Self {
        match error {
            MoveError::GameOver => HubError::GameOver,
            MoveError::NotYourTurn => HubError::NotYourTurn,
            MoveError::Illegal => HubError::Illegal,
        }
    }
}

/// Colour choice for a seek / engine game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    White,
    Black,
    Random,
}

impl ColorChoice {
    pub fn parse(text: &str) -> ColorChoice {
        match text {
            "w" | "white" => ColorChoice::White,
            "b" | "black" => ColorChoice::Black,
            _ => ColorChoice::Random,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            ColorChoice::White => "w",
            ColorChoice::Black => "b",
            ColorChoice::Random => "r",
        }
    }
}

/// Open seek for a human-vs-human game.
#[derive(Clone, Copy, Debug)]
pub struct Seek {
    pub id: u16,
    pub player: PlayerId,
    pub time_control: Option<TimeControl>,
    pub color: ColorChoice,
    pub created_ms: u64,
}

/// Kind of engine job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// Engine move in a game.
    Move,
    /// Hint for a human.
    Hint,
}

/// Engine job – everything the search needs (a copy; the hub may change meanwhile).
#[derive(Clone, Debug)]
pub struct EngineJob {
    pub game: GameId,
    /// Ply the job belongs to (if the game changes meanwhile, the result is discarded).
    pub ply: u16,
    pub kind: JobKind,
    pub demo: bool,
    pub root: Position,
    /// Hashes of previous positions (for repetition draws).
    pub history: Vec<u64, 128>,
    pub limits: SearchLimits,
}

/// Current engine statistics (for the display and the web app).
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineStats {
    /// Is the engine currently working on a job?
    pub thinking: bool,
    pub game: Option<GameId>,
    pub depth: u8,
    pub seldepth: u8,
    pub nodes: u64,
    /// Nodes per second.
    pub nps: u64,
    /// Score from White's point of view (centipawns) at the last completed depth.
    pub score_white: i32,
    /// Last completed search (shown while the engine is idle).
    pub last_depth: u8,
    pub last_nps: u64,
    /// Number of queued jobs.
    pub queue: u8,
    /// Principal variation of the last completed depth of an engine move (hints don't write
    /// here – the display shows it to everyone). Valid for game `pv_game` after `pv_ply` plies.
    pub pv: [Move; PV_LEN],
    pub pv_len: u8,
    pub pv_game: Option<GameId>,
    pub pv_ply: u16,
}

impl EngineStats {
    pub fn pv(&self) -> &[Move] {
        &self.pv[..self.pv_len as usize]
    }
}

/// Network info shown by the web app (set by the firmware / simulator).
#[derive(Clone, Debug, Default)]
pub struct NetInfo {
    /// "ap" = own access point only, "sta" = connected to the home network.
    pub mode: heapless::String<4>,
    pub ap_ssid: heapless::String<32>,
    pub sta_ssid: heapless::String<32>,
    /// IP address in the home network (empty when not connected).
    pub sta_ip: heapless::String<16>,
    /// Error of the last connection attempt (a code for translation).
    pub sta_error: heapless::String<16>,
    /// Is a connection attempt in progress?
    pub connecting: bool,
}

/// Firmware version and deployed model (filled by the platform at boot; shown in the web settings).
#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    /// Firmware version, e.g. "0.2.0 (8f37cbd)".
    pub firmware: heapless::String<40>,
    /// Chip the firmware runs on, e.g. "ESP32-S3, 240 MHz".
    pub chip: heapless::String<40>,
    /// Measured strength (Elo vs. Stockfish at 60+0.6 s), rounded down – the web app shows "2600+".
    pub elo: u16,
    /// NNUE network name (file name without extension), e.g. "h128-v3-i8".
    pub model: heapless::String<32>,
    /// Network architecture, e.g. "768→128×2→1, i8, 8 buckets".
    pub model_arch: heapless::String<48>,
    /// Network size in bytes.
    pub model_bytes: u32,
    /// Chip temperature in tenths of °C (built-in sensor; `None` = unknown). Updated at runtime.
    pub temp_dc: Option<i16>,
    /// Engine transposition table size in kB (0 = unknown) and whether it lives in PSRAM.
    pub hash_kb: u32,
    pub hash_psram: bool,
}

/// Complete server state.
pub struct Hub {
    players: Vec<Player, MAX_PLAYERS>,
    games: Vec<Game, MAX_GAMES>,
    seeks: Vec<Seek, MAX_SEEKS>,
    next_player_id: PlayerId,
    next_game_id: GameId,
    next_seek_id: u16,
    rng: u64,
    /// Job the engine is currently computing (game, ply, kind).
    current_job: Option<(GameId, u16, JobKind)>,
    /// Requested hints (game, ply).
    hint_requests: Vec<(GameId, u16), 4>,
    pub engine: EngineStats,
    pub net: NetInfo,
    pub system: SystemInfo,
    /// Token of the player allowed to change Wi-Fi settings.
    owner_token: Option<u64>,
    /// May the first player to log in claim ownership? (After a factory reset.)
    provisioning_open: bool,
    /// When the last demo game ended (for the pause before the next one).
    demo_finished_ms: Option<u64>,
    /// Enable the demo game (engine vs. engine).
    demo_enabled: bool,
    /// Incremented on lobby changes (players, seeks, games).
    lobby_version: u32,
    /// Peak concurrent online players since boot (for display stats).
    pub peak_online: u16,
    /// Number of human games started since boot.
    pub games_started: u32,
}

impl Hub {
    /// New hub. `seed` = random number (session tokens, colour draws).
    pub fn new(seed: u64) -> Hub {
        Hub {
            players: Vec::new(),
            games: Vec::new(),
            seeks: Vec::new(),
            next_player_id: 1,
            next_game_id: 1,
            next_seek_id: 1,
            rng: seed | 1,
            current_job: None,
            hint_requests: Vec::new(),
            engine: EngineStats::default(),
            net: NetInfo::default(),
            system: SystemInfo::default(),
            owner_token: None,
            provisioning_open: false,
            demo_finished_ms: None,
            demo_enabled: true,
            lobby_version: 1,
            peak_online: 0,
            games_started: 0,
        }
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Pseudo-random number (xorshift64*).
    pub fn random(&mut self) -> u64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn changed(&mut self) {
        self.lobby_version = self.lobby_version.wrapping_add(1);
    }

    pub fn lobby_version(&self) -> u32 {
        self.lobby_version
    }

    pub fn players(&self) -> &[Player] {
        &self.players
    }

    pub fn games(&self) -> &[Game] {
        &self.games
    }

    pub fn seeks(&self) -> &[Seek] {
        &self.seeks
    }

    pub fn game(&self, id: GameId) -> Option<&Game> {
        self.games.iter().find(|g| g.id == id)
    }

    fn game_mut(&mut self, id: GameId) -> Option<&mut Game> {
        self.games.iter_mut().find(|g| g.id == id)
    }

    pub fn player(&self, id: PlayerId) -> Option<&Player> {
        self.players.iter().find(|p| p.id == id)
    }

    /// Number of online players.
    pub fn online_count(&self, now: u64) -> usize {
        self.players
            .iter()
            .filter(|p| p.is_online(now, ONLINE_WINDOW_MS))
            .count()
    }

    /// The player's active (unfinished) game.
    pub fn active_game_of(&self, player: PlayerId) -> Option<&Game> {
        self.games
            .iter()
            .find(|g| g.is_active() && g.has_human(player))
    }

    /// The player's latest game (active, else most recently finished) – for returning after a page reload.
    pub fn current_game_of(&self, player: PlayerId) -> Option<&Game> {
        self.active_game_of(player).or_else(|| {
            self.games
                .iter()
                .filter(|g| g.has_human(player))
                .max_by_key(|g| g.last_activity_ms)
        })
    }

    /// Participant name (nickname; engine = `None`, the web app names it by level).
    pub fn participant_nick(&self, participant: Participant) -> Option<&str> {
        participant
            .human()
            .and_then(|id| self.player(id))
            .map(|p| p.nick.as_str())
    }

    // ------------------------------------------------------------------
    // Sessions and Wi-Fi owner
    // ------------------------------------------------------------------

    /// Opens/closes the possibility to claim ownership (Wi-Fi settings).
    /// The firmware calls `set_provisioning(true)` when no Wi-Fi is stored.
    pub fn set_provisioning(&mut self, open: bool) {
        self.provisioning_open = open;
        if open {
            self.owner_token = None;
        }
    }

    /// May this token change the Wi-Fi settings?
    pub fn is_owner(&self, token: u64) -> bool {
        self.owner_token == Some(token)
    }

    /// Login (or session resume). Returns (id, token).
    /// - With a known token: the session continues (the nickname may change).
    /// - Without a token: a new player with the given nickname.
    pub fn hello(
        &mut self,
        now: u64,
        token: Option<u64>,
        nick: &str,
    ) -> Result<(PlayerId, u64), HubError> {
        if let Some(token) = token
            && let Some(index) = self.players.iter().position(|p| p.token == token)
        {
            if !nick.is_empty() {
                let nick = validate_nick(nick)?;
                if self.nick_taken(&nick, Some(self.players[index].id)) {
                    return Err(HubError::NickTaken);
                }
                if self.players[index].nick != nick {
                    self.players[index].nick = nick;
                    self.changed();
                }
            }
            self.players[index].last_seen_ms = now;
            self.claim_owner(token);
            return Ok((self.players[index].id, token));
        }

        let nick: Nick = validate_nick(nick)?;
        if self.nick_taken(&nick, None) {
            return Err(HubError::NickTaken);
        }
        if self.players.is_full() {
            self.drop_stale_player(now);
        }
        let token = self.random();
        let id = self.next_player_id;
        self.next_player_id = self.next_player_id.wrapping_add(1).max(1);
        self.players
            .push(Player {
                id,
                token,
                nick,
                last_seen_ms: now,
                watching: None,
            })
            .map_err(|_| HubError::Full)?;
        self.claim_owner(token);
        self.changed();
        Ok((id, token))
    }

    /// The first player after a reset becomes the owner (may configure Wi-Fi).
    fn claim_owner(&mut self, token: u64) {
        if self.provisioning_open && self.owner_token.is_none() {
            self.owner_token = Some(token);
        }
    }

    fn nick_taken(&self, nick: &str, except: Option<PlayerId>) -> bool {
        self.players
            .iter()
            .any(|p| Some(p.id) != except && same_nick(&p.nick, nick))
    }

    /// Frees a slot: removes the longest-idle player without an active game.
    fn drop_stale_player(&mut self, now: u64) {
        let candidate = self
            .players
            .iter()
            .filter(|p| !p.is_online(now, ONLINE_WINDOW_MS) && self.active_game_of(p.id).is_none())
            .min_by_key(|p| p.last_seen_ms)
            .map(|p| p.id);
        if let Some(id) = candidate {
            self.remove_player(id);
        }
    }

    fn remove_player(&mut self, id: PlayerId) {
        self.players.retain(|p| p.id != id);
        self.seeks.retain(|s| s.player != id);
        self.changed();
    }

    /// Logout: resigns the running game, cancels the seek and the session (frees a slot).
    /// The Wi-Fi owner stays (their session is kept, only the game ends).
    pub fn logout(&mut self, now: u64, player: PlayerId) {
        if let Some(index) = self
            .games
            .iter()
            .position(|g| g.is_active() && g.has_human(player))
        {
            let game = &mut self.games[index];
            if let Some(color) = game.color_of(player) {
                game.resign(color, now);
            }
        }
        let is_owner = self
            .players
            .iter()
            .any(|p| p.id == player && Some(p.token) == self.owner_token);
        if is_owner {
            self.cancel_seek(player);
        } else {
            self.remove_player(player);
        }
    }

    /// Finds the player by token and records their activity.
    pub fn authenticate(&mut self, now: u64, token: Option<u64>) -> Result<PlayerId, HubError> {
        let token = token.ok_or(HubError::NoSession)?;
        let player = self
            .players
            .iter_mut()
            .find(|p| p.token == token)
            .ok_or(HubError::NoSession)?;
        let was_offline = !player.is_online(now, ONLINE_WINDOW_MS);
        player.last_seen_ms = now;
        let id = player.id;
        if was_offline {
            self.changed();
        }
        Ok(id)
    }

    /// Records which game the player is watching (for the spectator count).
    pub fn set_watching(&mut self, player: PlayerId, game: Option<GameId>) {
        if let Some(p) = self.players.iter_mut().find(|p| p.id == player)
            && p.watching != game
        {
            p.watching = game;
        }
    }

    /// Spectator count (online players watching but not playing the game).
    pub fn spectators(&self, game: &Game, now: u64) -> usize {
        self.players
            .iter()
            .filter(|p| {
                p.watching == Some(game.id)
                    && p.is_online(now, ONLINE_WINDOW_MS)
                    && !game.has_human(p.id)
            })
            .count()
    }

    // ------------------------------------------------------------------
    // Seeks and game creation
    // ------------------------------------------------------------------

    /// Creates a human-vs-human seek (one per player).
    pub fn create_seek(
        &mut self,
        now: u64,
        player: PlayerId,
        time_control: Option<TimeControl>,
        color: ColorChoice,
    ) -> Result<u16, HubError> {
        if self.active_game_of(player).is_some() {
            return Err(HubError::AlreadyPlaying);
        }
        self.seeks.retain(|s| s.player != player);
        let id = self.next_seek_id;
        self.next_seek_id = self.next_seek_id.wrapping_add(1).max(1);
        self.seeks
            .push(Seek {
                id,
                player,
                time_control,
                color,
                created_ms: now,
            })
            .map_err(|_| HubError::Full)?;
        self.changed();
        Ok(id)
    }

    pub fn cancel_seek(&mut self, player: PlayerId) {
        let before = self.seeks.len();
        self.seeks.retain(|s| s.player != player);
        if self.seeks.len() != before {
            self.changed();
        }
    }

    /// Accepting a seek → new game.
    pub fn accept_seek(
        &mut self,
        now: u64,
        player: PlayerId,
        seek_id: u16,
    ) -> Result<GameId, HubError> {
        let seek = *self
            .seeks
            .iter()
            .find(|s| s.id == seek_id)
            .ok_or(HubError::NoSeek)?;
        if seek.player == player {
            return Err(HubError::NotAllowed);
        }
        if self.active_game_of(player).is_some() || self.active_game_of(seek.player).is_some() {
            return Err(HubError::AlreadyPlaying);
        }
        let creator_white = match seek.color {
            ColorChoice::White => true,
            ColorChoice::Black => false,
            ColorChoice::Random => self.random() & 1 == 0,
        };
        let (white, black) = if creator_white {
            (Participant::Human(seek.player), Participant::Human(player))
        } else {
            (Participant::Human(player), Participant::Human(seek.player))
        };
        let id = self.create_game(now, white, black, seek.time_control)?;
        self.seeks
            .retain(|s| s.player != player && s.player != seek.player);
        Ok(id)
    }

    /// Game against the engine.
    pub fn play_engine(
        &mut self,
        now: u64,
        player: PlayerId,
        level: u8,
        color: ColorChoice,
        time_control: Option<TimeControl>,
    ) -> Result<GameId, HubError> {
        if self.active_game_of(player).is_some() {
            return Err(HubError::AlreadyPlaying);
        }
        let level = level.clamp(1, chess_engine::LEVEL_COUNT);
        let human_white = match color {
            ColorChoice::White => true,
            ColorChoice::Black => false,
            ColorChoice::Random => self.random() & 1 == 0,
        };
        let engine = Participant::Engine { level };
        let human = Participant::Human(player);
        let (white, black) = if human_white {
            (human, engine)
        } else {
            (engine, human)
        };
        self.cancel_seek(player);
        self.create_game(now, white, black, time_control)
    }

    fn create_game(
        &mut self,
        now: u64,
        white: Participant,
        black: Participant,
        time_control: Option<TimeControl>,
    ) -> Result<GameId, HubError> {
        if self.games.is_full() {
            self.evict_game(now);
        }
        let id = self.next_game_id;
        self.next_game_id = self.next_game_id.wrapping_add(1).max(1);
        let seed = self.random();
        let game = Game::new(id, white, black, time_control, now, seed);
        self.games.push(game).map_err(|_| HubError::Full)?;
        if !(white.is_engine() && black.is_engine()) {
            self.games_started = self.games_started.saturating_add(1);
        }
        self.changed();
        Ok(id)
    }

    /// Frees a slot for a new game: the demo first, then the oldest finished game.
    fn evict_game(&mut self, _now: u64) {
        if let Some(index) = self.games.iter().position(|g| g.demo) {
            self.games.remove(index);
            return;
        }
        if let Some(index) = self
            .games
            .iter()
            .enumerate()
            .filter(|(_, g)| !g.is_active())
            .min_by_key(|(_, g)| g.last_activity_ms)
            .map(|(i, _)| i)
        {
            self.games.remove(index);
        }
    }

    // ------------------------------------------------------------------
    // In-game actions
    // ------------------------------------------------------------------

    fn player_game(
        &mut self,
        player: PlayerId,
        game: GameId,
    ) -> Result<(&mut Game, Color), HubError> {
        let game = self.game_mut(game).ok_or(HubError::NoGame)?;
        let color = game.color_of(player).ok_or(HubError::NotPlayer)?;
        Ok((game, color))
    }

    /// Player move (UCI, e.g. "e2e4", "e7e8q").
    pub fn make_move(
        &mut self,
        now: u64,
        player: PlayerId,
        game: GameId,
        uci: &str,
    ) -> Result<(), HubError> {
        let (game, color) = self.player_game(player, game)?;
        if !game.is_active() {
            return Err(HubError::GameOver);
        }
        if game.side_to_move() != color {
            return Err(HubError::NotYourTurn);
        }
        let mv = parse_uci(&game.position, uci).ok_or(HubError::Illegal)?;
        game.play(mv, now)?;
        let finished = !game.is_active();
        if finished {
            self.changed();
        }
        self.changed();
        Ok(())
    }

    pub fn resign(&mut self, now: u64, player: PlayerId, game: GameId) -> Result<(), HubError> {
        let (game, color) = self.player_game(player, game)?;
        if !game.is_active() {
            return Err(HubError::GameOver);
        }
        game.resign(color, now);
        self.changed();
        Ok(())
    }

    /// Offer / accept a draw. The engine never accepts (it plays to the end).
    pub fn offer_draw(&mut self, now: u64, player: PlayerId, game: GameId) -> Result<(), HubError> {
        let (game, color) = self.player_game(player, game)?;
        if !game.is_active() {
            return Err(HubError::GameOver);
        }
        if game.players[color.opponent().index()].is_engine() {
            return Err(HubError::NotAllowed);
        }
        if game.offer_draw(color, now) {
            self.changed();
        }
        Ok(())
    }

    pub fn decline_draw(
        &mut self,
        now: u64,
        player: PlayerId,
        game: GameId,
    ) -> Result<(), HubError> {
        let (game, _) = self.player_game(player, game)?;
        game.decline_draw(now);
        Ok(())
    }

    /// Take back a move (engine games only).
    pub fn take_back(&mut self, now: u64, player: PlayerId, game: GameId) -> Result<(), HubError> {
        let (game, color) = self.player_game(player, game)?;
        if game.take_back(color, now) {
            Ok(())
        } else {
            Err(HubError::NotAllowed)
        }
    }

    /// Hint request (engine games only, when the player is to move).
    pub fn request_hint(&mut self, player: PlayerId, game: GameId) -> Result<(), HubError> {
        let (game, color) = self.player_game(player, game)?;
        if !game.is_active() || !game.is_vs_engine() || game.side_to_move() != color {
            return Err(HubError::NotAllowed);
        }
        let key = (game.id, game.ply());
        if !self.hint_requests.contains(&key) {
            self.hint_requests.push(key).map_err(|_| HubError::Full)?;
        }
        Ok(())
    }

    /// Rematch: immediately against the engine, against a human once both agree (colours swap).
    pub fn rematch(
        &mut self,
        now: u64,
        player: PlayerId,
        game_id: GameId,
    ) -> Result<Option<GameId>, HubError> {
        let (game, color) = self.player_game(player, game_id)?;
        if game.is_active() {
            return Err(HubError::NotAllowed);
        }
        let opponent = game.players[color.opponent().index()];
        let both_agree = opponent.is_engine() || game.rematch_offer == Some(color.opponent());
        if !both_agree {
            game.rematch_offer = Some(color);
            game.bump(now);
            return Ok(None);
        }
        let [white, black] = game.players;
        let control = game.clock.map(|c| c.control);
        if self.active_game_of(player).is_some()
            || opponent
                .human()
                .is_some_and(|id| self.active_game_of(id).is_some())
        {
            return Err(HubError::AlreadyPlaying);
        }
        let new_id = self.create_game(now, black, white, control)?;
        if let Some(old) = self.game_mut(game_id) {
            old.rematch_offer = None;
            old.bump(now);
        }
        Ok(Some(new_id))
    }

    // ------------------------------------------------------------------
    // Periodic maintenance
    // ------------------------------------------------------------------

    /// Call regularly (e.g. every second): clocks, abandoned games, cleanup, demo.
    pub fn tick(&mut self, now: u64) {
        let mut changed = false;
        let online = self.online_count(now).min(u16::MAX as usize) as u16;
        self.peak_online = self.peak_online.max(online);

        // Out of time?
        for game in self.games.iter_mut() {
            changed |= game.check_flag(now);
        }

        // Abandoned games: a player has been silent too long.
        for index in 0..self.games.len() {
            if !self.games[index].is_active() || self.games[index].demo {
                continue;
            }
            let absent = Color::ALL.into_iter().find(|&c| {
                self.games[index].players[c.index()]
                    .human()
                    .is_some_and(|id| {
                        self.player(id)
                            .is_none_or(|p| now.saturating_sub(p.last_seen_ms) > ABANDON_MS)
                    })
            });
            if let Some(color) = absent {
                let game = &mut self.games[index];
                let outcome = if game.moves.len() < 2 {
                    Outcome {
                        winner: None,
                        reason: EndReason::Aborted,
                    }
                } else {
                    Outcome {
                        winner: Some(color.opponent()),
                        reason: EndReason::Abandoned,
                    }
                };
                game.finish(now, outcome);
                changed = true;
            }
        }

        // Clean up old finished games.
        let before = self.games.len();
        self.games.retain(|g| {
            g.is_active() || g.demo || now.saturating_sub(g.last_activity_ms) < FINISHED_KEEP_MS
        });
        changed |= before != self.games.len();

        // Clean up inactive sessions (without an active game).
        let stale: Vec<PlayerId, MAX_PLAYERS> = self
            .players
            .iter()
            .filter(|p| {
                now.saturating_sub(p.last_seen_ms) > SESSION_TIMEOUT_MS
                    && self.active_game_of(p.id).is_none()
            })
            .map(|p| p.id)
            .collect();
        for id in stale {
            if self
                .owner_token
                .is_some_and(|t| self.players.iter().any(|p| p.id == id && p.token == t))
            {
                continue; // keep the Wi-Fi owner
            }
            self.remove_player(id);
        }

        // Seeks from offline players.
        let before = self.seeks.len();
        let players = &self.players;
        self.seeks.retain(|s| {
            players
                .iter()
                .any(|p| p.id == s.player && p.is_online(now, ONLINE_WINDOW_MS))
        });
        changed |= before != self.seeks.len();

        changed |= self.manage_demo(now);

        if changed {
            self.changed();
        }
    }

    /// Enables/disables the engine-vs-engine demo game.
    pub fn set_demo_enabled(&mut self, enabled: bool) {
        self.demo_enabled = enabled;
    }

    /// Manages the demo game: after it ends, waits and starts a new one.
    fn manage_demo(&mut self, now: u64) -> bool {
        if !self.demo_enabled {
            return false;
        }
        let demo = self.games.iter().find(|g| g.demo);
        match demo {
            Some(game) if game.is_active() => false,
            Some(game) => {
                let finished = *self.demo_finished_ms.get_or_insert(game.last_activity_ms);
                if now.saturating_sub(finished) >= DEMO_RESTART_MS {
                    self.games.retain(|g| !g.demo);
                    self.demo_finished_ms = None;
                    self.start_demo(now)
                } else {
                    false
                }
            }
            None => self.start_demo(now),
        }
    }

    fn start_demo(&mut self, now: u64) -> bool {
        if self.games.is_full() {
            return false; // humans take priority for slots
        }
        let engine = Participant::Engine { level: 0 };
        match self.create_game(now, engine, engine, None) {
            Ok(id) => {
                if let Some(game) = self.game_mut(id) {
                    game.demo = true;
                }
                true
            }
            Err(_) => false,
        }
    }

    /// The demo game (if any).
    pub fn demo_game(&self) -> Option<&Game> {
        self.games.iter().find(|g| g.demo)
    }

    // ------------------------------------------------------------------
    // Engine queue
    // ------------------------------------------------------------------

    /// Pending human jobs (engine moves in human games + hints).
    fn human_jobs_waiting(&self) -> usize {
        let moves = self
            .games
            .iter()
            .filter(|g| g.is_active() && !g.demo && g.to_move().is_engine())
            .count();
        moves + self.hint_requests.len()
    }

    /// Picks the next engine job (or `None` if there is nothing to do).
    /// Human games come first (longest waiting), then hints, then the demo.
    pub fn next_engine_job(&mut self, now: u64) -> Option<EngineJob> {
        // 1) Engine move in a human game – the one waiting longest.
        let waiting = self.human_jobs_waiting();
        let human_game = self
            .games
            .iter()
            .filter(|g| g.is_active() && !g.demo && g.to_move().is_engine())
            .min_by_key(|g| g.last_activity_ms)
            .map(|g| g.id);

        let job = if let Some(id) = human_game {
            let game = self.game(id)?;
            let Participant::Engine { level } = game.to_move() else {
                return None;
            };
            let mut limits = level_limits(level, game.seed);
            // With several players waiting, shorten the time (everyone gets a faster reply).
            if let Some(ms) = limits.move_time_ms.as_mut() {
                *ms = (*ms / waiting.max(1) as u64).max(300);
                // With a clock: use at most ~1/25 of the remaining time.
                if let Some(clock) = game.clock {
                    let side = game.side_to_move();
                    let left = clock.remaining(side, side, now);
                    *ms = (*ms)
                        .min(left / 25 + clock.control.increment_ms / 2)
                        .max(100);
                }
            }
            Some(self.make_job(id, JobKind::Move, limits))
        } else if let Some(&(id, ply)) = self.hint_requests.first() {
            // 2) Hint.
            match self.game(id) {
                Some(game) if game.is_active() && game.ply() == ply => {
                    let mut limits = SearchLimits::move_time(HINT_MS);
                    limits.min_depth = HINT_MIN_DEPTH;
                    limits.hard_time_ms = Some(HINT_HARD_MS);
                    limits.noise_seed = game.seed;
                    Some(self.make_job(id, JobKind::Hint, limits))
                }
                _ => {
                    self.hint_requests.remove(0);
                    None
                }
            }
        } else {
            // 3) Demo game. While someone is online, only occasionally and briefly (web
            //    responsiveness first – the engine would compete with networking for CPU and flash cache).
            let busy = self.online_count(now) > 0;
            let move_ms = if busy {
                DEMO_MOVE_BUSY_MS
            } else {
                DEMO_MOVE_MS
            };
            self.demo_game()
                .filter(|g| g.is_active())
                .filter(|g| {
                    let pause = match (busy, g.ply()) {
                        (true, _) => DEMO_PAUSE_BUSY_MS,
                        (false, 0) => 0,
                        (false, _) => DEMO_PAUSE_IDLE_MS,
                    };
                    now.saturating_sub(g.last_activity_ms) >= pause
                })
                .map(|g| g.id)
                .map(|id| self.make_job(id, JobKind::Move, SearchLimits::move_time(move_ms)))
        };

        if let Some(job) = &job {
            self.current_job = Some((job.game, job.ply, job.kind));
            self.engine.thinking = true;
            self.engine.game = Some(job.game);
            self.engine.depth = 0;
            self.engine.nodes = 0;
            self.engine.queue = waiting.saturating_sub(1).min(255) as u8;
        }
        job
    }

    fn make_job(&self, id: GameId, kind: JobKind, limits: SearchLimits) -> EngineJob {
        let game = self.game(id).expect("game exists");
        EngineJob {
            game: id,
            ply: game.ply(),
            kind,
            demo: game.demo,
            root: game.position.clone(),
            history: game.history_hashes(),
            limits,
        }
    }

    /// Is the job obsolete? (The game ended/changed, or the demo must yield to a human.)
    /// The engine polls this during the search.
    pub fn job_obsolete(&self, job: &EngineJob) -> bool {
        let Some(game) = self.game(job.game) else {
            return true;
        };
        if !game.is_active() || game.ply() != job.ply {
            return true;
        }
        job.demo && self.human_jobs_waiting() > 0
    }

    /// Progress statistics from the engine.
    pub fn engine_progress(&mut self, depth: u8, seldepth: u8, nodes: u64, nps: u64) {
        self.engine.depth = depth;
        self.engine.seldepth = seldepth;
        self.engine.nodes = nodes;
        self.engine.nps = nps;
    }

    /// Completed depth (score from the side to move in the job's root position, principal variation).
    pub fn engine_iteration(&mut self, job: &EngineJob, depth: u8, score_stm: i32, pv: &[Move]) {
        self.engine.depth = depth;
        if job.kind == JobKind::Move {
            let len = pv.len().min(PV_LEN);
            self.engine.pv[..len].copy_from_slice(&pv[..len]);
            self.engine.pv_len = len as u8;
            self.engine.pv_game = Some(job.game);
            self.engine.pv_ply = job.ply;
        }
        self.engine.score_white = if job.root.side_to_move() == Color::White {
            score_stm
        } else {
            -score_stm
        };
    }

    /// Search result. `best` = `Move::NULL` if the engine found nothing / was interrupted.
    pub fn engine_done(&mut self, now: u64, job: &EngineJob, best: Move, depth: u8, nps: u64) {
        self.current_job = None;
        self.engine.thinking = false;
        self.engine.last_depth = depth;
        self.engine.last_nps = nps;
        if best.is_null() || self.job_obsolete(job) {
            return;
        }
        match job.kind {
            JobKind::Hint => {
                self.hint_requests.retain(|&(g, _)| g != job.game);
                if let Some(game) = self.game_mut(job.game) {
                    game.hint = Some((job.ply, best));
                    game.bump(now);
                }
            }
            JobKind::Move => {
                if let Some(game) = self.game_mut(job.game)
                    && game.to_move().is_engine()
                {
                    let _ = game.play(best, now);
                    let finished = !game.is_active();
                    if finished && game.demo {
                        self.demo_finished_ms = Some(now);
                    }
                    self.changed();
                }
            }
        }
    }

    /// Is the engine currently computing a move in this game?
    pub fn is_engine_thinking_on(&self, game: GameId) -> bool {
        matches!(self.current_job, Some((g, _, JobKind::Move)) if g == game)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub_with_two_players() -> (Hub, PlayerId, PlayerId) {
        let mut hub = Hub::new(42);
        hub.set_demo_enabled(false);
        let (a, _) = hub.hello(0, None, "Alice").unwrap();
        let (b, _) = hub.hello(0, None, "Bob").unwrap();
        (hub, a, b)
    }

    #[test]
    fn sessions_and_nicks() {
        let mut hub = Hub::new(1);
        let (id, token) = hub.hello(0, None, "Jana").unwrap();
        assert_eq!(hub.hello(0, None, "jana"), Err(HubError::NickTaken));
        assert_eq!(hub.hello(0, None, "x"), Err(HubError::BadNick));
        // Resume the session with the same token.
        assert_eq!(hub.hello(5, Some(token), "").unwrap(), (id, token));
        // Nickname change.
        hub.hello(6, Some(token), "Jana N").unwrap();
        assert_eq!(hub.player(id).unwrap().nick.as_str(), "Jana N");
        assert_eq!(hub.authenticate(7, Some(token)), Ok(id));
        assert_eq!(hub.authenticate(7, Some(123)), Err(HubError::NoSession));
    }

    #[test]
    fn first_player_after_reset_becomes_owner() {
        let mut hub = Hub::new(1);
        hub.set_provisioning(true);
        let (_, first) = hub.hello(0, None, "First").unwrap();
        let (_, second) = hub.hello(0, None, "Second").unwrap();
        assert!(hub.is_owner(first));
        assert!(!hub.is_owner(second));
        // Without open provisioning, nobody is the owner.
        let mut hub = Hub::new(1);
        let (_, token) = hub.hello(0, None, "Someone").unwrap();
        assert!(!hub.is_owner(token));
    }

    #[test]
    fn seek_accept_and_play() {
        let (mut hub, a, b) = hub_with_two_players();
        let seek = hub
            .create_seek(0, a, Some(TimeControl::new(5, 3)), ColorChoice::White)
            .unwrap();
        assert_eq!(hub.accept_seek(0, a, seek), Err(HubError::NotAllowed));
        let game = hub.accept_seek(0, b, seek).unwrap();
        assert!(hub.seeks().is_empty());
        assert_eq!(
            hub.make_move(0, b, game, "e7e5"),
            Err(HubError::NotYourTurn)
        );
        hub.make_move(0, a, game, "e2e4").unwrap();
        hub.make_move(0, b, game, "e7e5").unwrap();
        assert_eq!(hub.make_move(0, a, game, "e4e5"), Err(HubError::Illegal));
        // One player = one active game.
        assert_eq!(
            hub.play_engine(0, a, 3, ColorChoice::White, None),
            Err(HubError::AlreadyPlaying)
        );
        hub.resign(0, b, game).unwrap();
        assert_eq!(
            hub.game(game).unwrap().outcome.unwrap().winner,
            Some(Color::White)
        );
    }

    #[test]
    fn rematch_needs_both_humans() {
        let (mut hub, a, b) = hub_with_two_players();
        let seek = hub.create_seek(0, a, None, ColorChoice::White).unwrap();
        let game = hub.accept_seek(0, b, seek).unwrap();
        hub.resign(0, a, game).unwrap();
        assert_eq!(hub.rematch(0, a, game), Ok(None));
        let new = hub.rematch(0, b, game).unwrap().unwrap();
        // Colours swapped.
        assert_eq!(hub.game(new).unwrap().players[0], Participant::Human(b));
    }

    #[test]
    fn engine_jobs_prefer_humans_over_demo() {
        let mut hub = Hub::new(3);
        hub.tick(0); // starts the demo
        let demo_job = hub.next_engine_job(0).unwrap();
        assert!(demo_job.demo);
        assert!(!hub.job_obsolete(&demo_job));

        let (alice, _) = hub.hello(0, None, "Alice").unwrap();
        let game = hub
            .play_engine(0, alice, 5, ColorChoice::Black, None)
            .unwrap();
        // A human waits for an engine move → the demo job is obsolete.
        assert!(hub.job_obsolete(&demo_job));
        hub.engine_done(0, &demo_job, Move::NULL, 0, 0);

        let job = hub.next_engine_job(0).unwrap();
        assert_eq!(job.game, game);
        assert!(!job.demo);
        assert_eq!(job.limits.move_time_ms, Some(1_000));
        let mv = chess_core::legal_moves(&job.root).as_slice()[0];
        hub.engine_done(10, &job, mv, 5, 1000);
        assert_eq!(hub.game(game).unwrap().ply(), 1);
        // Now the human is to move → the engine plays the demo, but since a human is online,
        // only after a pause and briefly (web responsiveness first).
        assert!(hub.next_engine_job(10).is_none());
        let demo = hub.next_engine_job(DEMO_PAUSE_BUSY_MS).unwrap();
        assert!(demo.demo);
        assert_eq!(demo.limits.move_time_ms, Some(DEMO_MOVE_BUSY_MS));
    }

    #[test]
    fn demo_is_throttled_while_people_online() {
        let mut hub = Hub::new(5);
        hub.tick(0);
        // Nobody online → demo immediately with full time.
        let job = hub.next_engine_job(0).unwrap();
        assert!(job.demo);
        assert_eq!(job.limits.move_time_ms, Some(DEMO_MOVE_MS));
        let mv = chess_core::legal_moves(&job.root).as_slice()[0];
        hub.engine_done(100, &job, mv, 5, 1000);

        // Someone connects → next demo move only after a pause since the last move, with less time.
        hub.hello(200, None, "Bob").unwrap();
        assert!(hub.next_engine_job(200).is_none());
        assert!(hub.next_engine_job(100 + DEMO_PAUSE_BUSY_MS - 1).is_none());
        let job = hub.next_engine_job(100 + DEMO_PAUSE_BUSY_MS).unwrap();
        assert!(job.demo);
        assert_eq!(job.limits.move_time_ms, Some(DEMO_MOVE_BUSY_MS));
        hub.engine_done(100 + DEMO_PAUSE_BUSY_MS, &job, Move::NULL, 0, 0);

        // After they leave (outside the online window) full time again (the pause has passed).
        let later = 200 + ONLINE_WINDOW_MS + 1;
        assert!(later - 100 >= DEMO_PAUSE_IDLE_MS);
        let job = hub.next_engine_job(later).unwrap();
        assert_eq!(job.limits.move_time_ms, Some(DEMO_MOVE_MS));
    }

    #[test]
    fn demo_pauses_between_moves_when_nobody_online() {
        let mut hub = Hub::new(5);
        hub.tick(0);
        // First move of a new demo game immediately.
        let job = hub.next_engine_job(0).unwrap();
        assert!(job.demo);
        let mv = chess_core::legal_moves(&job.root).as_slice()[0];
        hub.engine_done(DEMO_MOVE_MS, &job, mv, 5, 1000);
        // The next one only after a pause (the chip rests between moves).
        assert!(hub.next_engine_job(DEMO_MOVE_MS).is_none());
        assert!(
            hub.next_engine_job(DEMO_MOVE_MS + DEMO_PAUSE_IDLE_MS - 1)
                .is_none()
        );
        let job = hub
            .next_engine_job(DEMO_MOVE_MS + DEMO_PAUSE_IDLE_MS)
            .unwrap();
        assert!(job.demo);
        assert_eq!(job.limits.move_time_ms, Some(DEMO_MOVE_MS));
    }

    #[test]
    fn obsolete_job_result_is_ignored() {
        let mut hub = Hub::new(3);
        hub.set_demo_enabled(false);
        let (alice, _) = hub.hello(0, None, "Alice").unwrap();
        let game = hub
            .play_engine(0, alice, 2, ColorChoice::Black, None)
            .unwrap();
        let job = hub.next_engine_job(0).unwrap();
        hub.resign(0, alice, game).unwrap();
        assert!(hub.job_obsolete(&job));
        let mv = chess_core::legal_moves(&job.root).as_slice()[0];
        hub.engine_done(0, &job, mv, 3, 0);
        assert_eq!(hub.game(game).unwrap().ply(), 0);
    }

    #[test]
    fn hint_flow() {
        let mut hub = Hub::new(3);
        hub.set_demo_enabled(false);
        let (alice, _) = hub.hello(0, None, "Alice").unwrap();
        let game = hub
            .play_engine(0, alice, 2, ColorChoice::White, None)
            .unwrap();
        hub.request_hint(alice, game).unwrap();
        let job = hub.next_engine_job(0).unwrap();
        assert_eq!(job.kind, JobKind::Hint);
        let mv = chess_core::legal_moves(&job.root).as_slice()[0];
        hub.engine_done(0, &job, mv, 3, 0);
        assert_eq!(hub.game(game).unwrap().hint, Some((0, mv)));
        assert_eq!(hub.game(game).unwrap().ply(), 0); // a hint doesn't play a move
    }

    #[test]
    fn hint_searches_at_least_min_depth() {
        let mut hub = Hub::new(3);
        hub.set_demo_enabled(false);
        let (alice, _) = hub.hello(0, None, "Alice").unwrap();
        let game = hub
            .play_engine(0, alice, 2, ColorChoice::White, None)
            .unwrap();
        hub.request_hint(alice, game).unwrap();
        let job = hub.next_engine_job(0).unwrap();
        assert_eq!(job.limits.min_depth, HINT_MIN_DEPTH);
        const { assert!(HINT_MIN_DEPTH >= 8) };
        assert_eq!(job.limits.hard_time_ms, Some(HINT_HARD_MS));
    }

    #[test]
    fn no_hint_between_humans() {
        // Fair play: no hints in human-vs-human games (for either side).
        let (mut hub, a, b) = hub_with_two_players();
        let seek = hub.create_seek(0, a, None, ColorChoice::White).unwrap();
        let game = hub.accept_seek(0, b, seek).unwrap();
        assert_eq!(hub.request_hint(a, game), Err(HubError::NotAllowed));
        hub.make_move(0, a, game, "e2e4").unwrap();
        assert_eq!(hub.request_hint(b, game), Err(HubError::NotAllowed));
        assert!(hub.next_engine_job(0).is_none());
    }

    #[test]
    fn pv_is_kept_for_engine_moves_only() {
        let mut hub = Hub::new(3);
        hub.set_demo_enabled(false);
        let (alice, _) = hub.hello(0, None, "Alice").unwrap();
        let game = hub
            .play_engine(0, alice, 5, ColorChoice::Black, None)
            .unwrap();
        // Engine move (white) → the PV is stored.
        let job = hub.next_engine_job(0).unwrap();
        assert_eq!(job.kind, JobKind::Move);
        let pv = [parse_uci(&job.root, "e2e4").unwrap()];
        hub.engine_iteration(&job, 1, 20, &pv);
        assert_eq!(hub.engine.pv(), &pv);
        assert_eq!((hub.engine.pv_game, hub.engine.pv_ply), (Some(game), 0));
        hub.engine_done(0, &job, pv[0], 1, 0);
        // Hint for the human → PV unchanged (the display shows it to everyone).
        hub.request_hint(alice, game).unwrap();
        let hint = hub.next_engine_job(0).unwrap();
        assert_eq!(hint.kind, JobKind::Hint);
        let hint_pv = [parse_uci(&hint.root, "e7e5").unwrap()];
        hub.engine_iteration(&hint, 1, 0, &hint_pv);
        assert_eq!(hub.engine.pv(), &pv);
        assert_eq!(hub.engine.pv_ply, 0);
    }

    #[test]
    fn abandoned_game_and_session_cleanup() {
        let (mut hub, a, b) = hub_with_two_players();
        let seek = hub.create_seek(0, a, None, ColorChoice::White).unwrap();
        let game = hub.accept_seek(0, b, seek).unwrap();
        hub.make_move(0, a, game, "e2e4").unwrap();
        hub.make_move(0, b, game, "e7e5").unwrap();
        // Alice keeps polling, Bob is gone.
        let token_a = hub.player(a).unwrap().token;
        hub.authenticate(ABANDON_MS + 1, Some(token_a)).unwrap();
        hub.tick(ABANDON_MS + 1);
        let outcome = hub.game(game).unwrap().outcome.unwrap();
        assert_eq!(outcome.reason, EndReason::Abandoned);
        assert_eq!(outcome.winner, Some(Color::White));
        // After a long time Bob's session is dropped.
        hub.tick(SESSION_TIMEOUT_MS + ABANDON_MS + 10);
        assert!(hub.player(b).is_none());
    }

    #[test]
    fn demo_restarts_after_finish() {
        let mut hub = Hub::new(9);
        hub.tick(0);
        let id = hub.demo_game().unwrap().id;
        let game = hub.game_mut(id).unwrap();
        game.finish(
            100,
            Outcome {
                winner: None,
                reason: EndReason::Stalemate,
            },
        );
        hub.tick(200);
        assert_eq!(hub.demo_game().unwrap().id, id);
        hub.tick(100 + DEMO_RESTART_MS + 1);
        let new = hub.demo_game().unwrap();
        assert_ne!(new.id, id);
        assert!(new.is_active());
    }

    #[test]
    fn engine_time_shrinks_with_queue() {
        let mut hub = Hub::new(5);
        hub.set_demo_enabled(false);
        let (a, _) = hub.hello(0, None, "Alice").unwrap();
        let (b, _) = hub.hello(0, None, "Bob").unwrap();
        hub.play_engine(0, a, 8, ColorChoice::Black, None).unwrap();
        hub.play_engine(1, b, 8, ColorChoice::Black, None).unwrap();
        let job = hub.next_engine_job(2).unwrap();
        assert_eq!(job.limits.move_time_ms, Some(5_000)); // 10 s / 2 waiting
    }
}
