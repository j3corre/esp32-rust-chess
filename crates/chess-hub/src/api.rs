//! REST API on top of the hub: `/api/...` → JSON.
//!
//! [`handle`] takes the path, query parameters and time, performs the action
//! and writes the JSON response into `out`. The HTTP server (picoserve on the
//! board and on the PC) just forwards requests here – all logic is testable
//! without a network.
//!
//! All parameters are in the URL (`?t=token&id=5&m=e2e4`), even for POST.

use core::fmt::{self, Write};

use chess_core::{Color, legal_moves};
use heapless::String;

use crate::game::{Game, Participant, TimeControl};
use crate::hub::{ColorChoice, Hub, HubError};
use crate::json;
use crate::params;
use crate::player::PlayerId;

/// Maximum number of moves per response (the web app fetches the rest with another request).
pub const MOVES_PER_RESPONSE: usize = 120;

/// What the HTTP layer should do after handling a request.
#[derive(Debug, PartialEq, Eq)]
pub enum ApiOutcome {
    /// The response is written to `out`; just send it (200, application/json).
    Json,
    /// Unknown path (404).
    NotFound,
    /// The owner wants to save Wi-Fi credentials – the platform stores them and connects.
    /// (The JSON response `{"ok":true}` is already written.)
    WifiSave {
        ssid: String<32>,
        password: String<64>,
    },
    /// The owner wants to forget the Wi-Fi network (back to AP mode).
    WifiForget,
}

/// Handles an API request. `path` without the query (e.g. "/api/lobby"), `query` without "?".
pub fn handle<W: Write>(
    hub: &mut Hub,
    now: u64,
    path: &str,
    query: &str,
    out: &mut W,
) -> Result<ApiOutcome, fmt::Error> {
    let token = params::token(query);
    let game_id: Option<u16> = params::number(query, "id");

    // Actions that return only {"ok":…}.
    macro_rules! action {
        ($body:expr) => {{
            let result: Result<(), HubError> = (|| $body)();
            json::result(out, result.map_err(HubError::code))?;
            return Ok(ApiOutcome::Json);
        }};
    }

    match path {
        "/api/hello" => {
            let nick: String<96> = params::text(query, "nick").unwrap_or_default();
            match hub.hello(now, token, &nick) {
                Ok((id, token)) => {
                    let nick = hub.player(id).map(|p| p.nick.clone()).unwrap_or_default();
                    write!(
                        out,
                        "{{\"ok\":true,\"id\":{id},\"token\":\"{token:016x}\",\"nick\":"
                    )?;
                    json::string(out, &nick)?;
                    write!(out, ",\"owner\":{}}}", hub.is_owner(token))?;
                }
                Err(error) => json::result(out, Err(error.code()))?,
            }
            Ok(ApiOutcome::Json)
        }
        "/api/lobby" => {
            let me = hub.authenticate(now, token).ok();
            if let Some(me) = me {
                hub.set_watching(me, None);
            }
            write_lobby(hub, now, me, token, out)?;
            Ok(ApiOutcome::Json)
        }
        "/api/game" => {
            let Some(id) = game_id else {
                json::result(out, Err(HubError::BadRequest.code()))?;
                return Ok(ApiOutcome::Json);
            };
            let me = hub.authenticate(now, token).ok();
            if let Some(me) = me {
                let playing = hub.game(id).is_some_and(|g| g.has_human(me));
                hub.set_watching(me, if playing { None } else { Some(id) });
            }
            let since: usize = params::number(query, "since").unwrap_or(0);
            match hub.game(id) {
                Some(game) => write_game(hub, game, now, me, since, out)?,
                None => json::result(out, Err(HubError::NoGame.code()))?,
            }
            Ok(ApiOutcome::Json)
        }
        "/api/seek" => action!({
            let me = hub.authenticate(now, token)?;
            let tc = parse_time_control(query)?;
            let color = ColorChoice::parse(params::raw(query, "color").unwrap_or("r"));
            hub.create_seek(now, me, tc, color).map(|_| ())
        }),
        "/api/bye" => action!({
            let me = hub.authenticate(now, token)?;
            hub.logout(now, me);
            Ok(())
        }),
        "/api/cancel" => action!({
            let me = hub.authenticate(now, token)?;
            hub.cancel_seek(me);
            Ok(())
        }),
        "/api/accept" => {
            let result = (|| {
                let me = hub.authenticate(now, token)?;
                let seek = params::number(query, "seek").ok_or(HubError::BadRequest)?;
                hub.accept_seek(now, me, seek)
            })();
            write_game_created(out, result)?;
            Ok(ApiOutcome::Json)
        }
        "/api/engine" => {
            let result = (|| {
                let me = hub.authenticate(now, token)?;
                let level = params::number(query, "level").unwrap_or(3);
                let color = ColorChoice::parse(params::raw(query, "color").unwrap_or("r"));
                let tc = parse_time_control(query)?;
                hub.play_engine(now, me, level, color, tc)
            })();
            write_game_created(out, result)?;
            Ok(ApiOutcome::Json)
        }
        "/api/move" => action!({
            let me = hub.authenticate(now, token)?;
            let id = game_id.ok_or(HubError::BadRequest)?;
            let uci: String<8> = params::text(query, "m").ok_or(HubError::BadRequest)?;
            hub.make_move(now, me, id, &uci)
        }),
        "/api/resign" => action!({
            let me = hub.authenticate(now, token)?;
            hub.resign(now, me, game_id.ok_or(HubError::BadRequest)?)
        }),
        "/api/draw" => action!({
            let me = hub.authenticate(now, token)?;
            let id = game_id.ok_or(HubError::BadRequest)?;
            if params::raw(query, "decline").is_some() {
                hub.decline_draw(now, me, id)
            } else {
                hub.offer_draw(now, me, id)
            }
        }),
        "/api/takeback" => action!({
            let me = hub.authenticate(now, token)?;
            hub.take_back(now, me, game_id.ok_or(HubError::BadRequest)?)
        }),
        "/api/hint" => action!({
            let me = hub.authenticate(now, token)?;
            hub.request_hint(me, game_id.ok_or(HubError::BadRequest)?)
        }),
        "/api/rematch" => {
            let result = (|| {
                let me = hub.authenticate(now, token)?;
                hub.rematch(now, me, game_id.ok_or(HubError::BadRequest)?)
            })();
            match result {
                Ok(Some(id)) => write!(out, "{{\"ok\":true,\"game\":{id}}}")?,
                Ok(None) => write!(out, "{{\"ok\":true,\"game\":null}}")?,
                Err(error) => json::result(out, Err(error.code()))?,
            }
            Ok(ApiOutcome::Json)
        }
        "/api/wifi" => {
            // Network status (anyone), save / forget (owner only).
            let owner = token.is_some_and(|t| hub.is_owner(t));
            if let Some(ssid) = params::text::<32>(query, "ssid") {
                if !owner {
                    json::result(out, Err(HubError::NotAllowed.code()))?;
                    return Ok(ApiOutcome::Json);
                }
                let password: String<64> = params::text(query, "pass").unwrap_or_default();
                let valid = !ssid.is_empty() && (password.is_empty() || password.len() >= 8);
                if !valid {
                    json::result(out, Err(HubError::BadRequest.code()))?;
                    return Ok(ApiOutcome::Json);
                }
                json::result(out, Ok(()))?;
                return Ok(ApiOutcome::WifiSave { ssid, password });
            }
            if params::raw(query, "forget").is_some() {
                if !owner {
                    json::result(out, Err(HubError::NotAllowed.code()))?;
                    return Ok(ApiOutcome::Json);
                }
                json::result(out, Ok(()))?;
                return Ok(ApiOutcome::WifiForget);
            }
            write_net(hub, owner, out)?;
            Ok(ApiOutcome::Json)
        }
        _ => Ok(ApiOutcome::NotFound),
    }
}

fn write_game_created<W: Write>(out: &mut W, result: Result<u16, HubError>) -> fmt::Result {
    match result {
        Ok(id) => write!(out, "{{\"ok\":true,\"game\":{id}}}"),
        Err(error) => json::result(out, Err(error.code())),
    }
}

/// Time control from `min` and `inc` (no `min` = no clock).
fn parse_time_control(query: &str) -> Result<Option<TimeControl>, HubError> {
    match params::number::<u64>(query, "min") {
        None | Some(0) => Ok(None),
        Some(minutes) => {
            let increment = params::number(query, "inc").unwrap_or(0);
            TimeControl::from_minutes(minutes, increment)
                .map(Some)
                .ok_or(HubError::BadRequest)
        }
    }
}

fn color_code(color: Color) -> &'static str {
    match color {
        Color::White => "w",
        Color::Black => "b",
    }
}

/// `{"name":"Jan","engine":false,"level":0,"id":3}`
fn write_participant<W: Write>(
    hub: &Hub,
    participant: Participant,
    now: u64,
    out: &mut W,
) -> fmt::Result {
    match participant {
        Participant::Human(id) => {
            out.write_str("{\"name\":")?;
            json::opt_str(out, hub.participant_nick(participant))?;
            let online = hub
                .player(id)
                .is_some_and(|p| p.is_online(now, crate::hub::ONLINE_WINDOW_MS));
            write!(out, ",\"engine\":false,\"id\":{id},\"online\":{online}}}")
        }
        Participant::Engine { level } => {
            write!(out, "{{\"name\":null,\"engine\":true,\"level\":{level}}}")
        }
    }
}

/// Lobby participant (omits `name:null` for the engine to save space).
fn write_lobby_participant<W: Write>(
    hub: &Hub,
    participant: Participant,
    now: u64,
    out: &mut W,
) -> fmt::Result {
    match participant {
        Participant::Engine { level } => write!(out, "{{\"engine\":true,\"level\":{level}}}"),
        human => write_participant(hub, human, now, out),
    }
}

fn write_status<W: Write>(game: &Game, out: &mut W) -> fmt::Result {
    match game.outcome {
        None => out.write_str("\"status\":\"active\",\"result\":\"*\",\"winner\":null"),
        Some(outcome) => {
            write!(
                out,
                "\"status\":\"{}\",\"result\":\"{}\",\"winner\":",
                outcome.reason.code(),
                outcome.result_text()
            )?;
            match outcome.winner {
                Some(color) => write!(out, "\"{}\"", color_code(color)),
                None => out.write_str("null"),
            }
        }
    }
}

/// Lobby: me, players, seeks, games, engine, network.
pub fn write_lobby<W: Write>(
    hub: &Hub,
    now: u64,
    me: Option<PlayerId>,
    token: Option<u64>,
    out: &mut W,
) -> fmt::Result {
    write!(out, "{{\"v\":{},\"me\":", hub.lobby_version())?;
    match me.and_then(|id| hub.player(id)) {
        Some(player) => {
            write!(out, "{{\"id\":{},\"nick\":", player.id)?;
            json::string(out, &player.nick)?;
            let owner = token.is_some_and(|t| hub.is_owner(t));
            write!(out, ",\"owner\":{owner},\"game\":")?;
            match hub.active_game_of(player.id) {
                Some(game) => write!(out, "{}", game.id)?,
                None => out.write_str("null")?,
            }
            let seeking = hub.seeks().iter().any(|s| s.player == player.id);
            write!(out, ",\"seeking\":{seeking}}}")?;
        }
        None => out.write_str("null")?,
    }

    out.write_str(",\"players\":[")?;
    let mut first = true;
    for player in hub
        .players()
        .iter()
        .filter(|p| p.is_online(now, crate::hub::ONLINE_WINDOW_MS))
    {
        if !first {
            out.write_char(',')?;
        }
        first = false;
        write!(out, "{{\"id\":{},\"nick\":", player.id)?;
        json::string(out, &player.nick)?;
        let status = if hub.active_game_of(player.id).is_some() {
            "playing"
        } else if hub.seeks().iter().any(|s| s.player == player.id) {
            "seeking"
        } else {
            "idle"
        };
        write!(out, ",\"status\":\"{status}\"}}")?;
    }

    out.write_str("],\"seeks\":[")?;
    for (i, seek) in hub.seeks().iter().enumerate() {
        if i > 0 {
            out.write_char(',')?;
        }
        write!(out, "{{\"id\":{},\"by\":{},\"nick\":", seek.id, seek.player)?;
        json::opt_str(out, hub.player(seek.player).map(|p| p.nick.as_str()))?;
        match seek.time_control {
            Some(tc) => write!(
                out,
                ",\"min\":{},\"inc\":{}",
                tc.base_ms / 60_000,
                tc.increment_ms / 1000
            )?,
            None => out.write_str(",\"min\":0,\"inc\":0")?,
        }
        write!(out, ",\"color\":\"{}\"}}", seek.color.code())?;
    }

    out.write_str("],\"games\":[")?;
    // Active games first, then finished ones (newest first).
    let mut first = true;
    for active in [true, false] {
        for game in hub.games().iter().filter(|g| g.is_active() == active) {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            // The lobby is the longest response (it must fit the web server's JSON buffer),
            // so default values are omitted: demo:false, last:null, viewers:0,
            // name:null for the engine and result/winner for running games.
            write!(out, "{{\"id\":{}", game.id)?;
            if game.demo {
                out.write_str(",\"demo\":true")?;
            }
            out.write_str(",\"w\":")?;
            write_lobby_participant(hub, game.players[0], now, out)?;
            out.write_str(",\"b\":")?;
            write_lobby_participant(hub, game.players[1], now, out)?;
            out.write_str(",\"board\":\"")?;
            game.position.write_board_fen(out)?;
            write!(
                out,
                "\",\"turn\":\"{}\",\"ply\":{}",
                color_code(game.side_to_move()),
                game.ply()
            )?;
            if let Some(mv) = game.last_move() {
                write!(out, ",\"last\":\"{mv}\"")?;
            }
            let viewers = hub.spectators(game, now);
            if viewers > 0 {
                write!(out, ",\"viewers\":{viewers}")?;
            }
            out.write_char(',')?;
            if game.outcome.is_none() {
                out.write_str("\"status\":\"active\"")?;
            } else {
                write_status(game, out)?;
            }
            out.write_char('}')?;
        }
    }

    let engine = &hub.engine;
    write!(
        out,
        "],\"engine\":{{\"thinking\":{},\"game\":{},\"depth\":{},\"nps\":{},\"queue\":{},\"lastDepth\":{},\"lastNps\":{}}},",
        engine.thinking,
        engine.game.map_or(0, |g| g as i32),
        engine.depth,
        engine.nps,
        engine.queue,
        engine.last_depth,
        engine.last_nps,
    )?;
    out.write_str("\"online\":")?;
    write!(out, "{},", hub.online_count(now))?;
    out.write_str("\"net\":")?;
    write_net(hub, token.is_some_and(|t| hub.is_owner(t)), out)?;
    out.write_str(",\"sys\":")?;
    write_system(hub, now, out)?;
    out.write_char('}')
}

/// Version and model: `{"fw":"0.2.0 (8f37cbd)","chip":"ESP32-C6, 160 MHz","elo":2550,
/// "model":"h128-v3-i8","arch":"…","bytes":99200,"temp":47.5,"uptime":123}` (temp may be null)
fn write_system<W: Write>(hub: &Hub, now: u64, out: &mut W) -> fmt::Result {
    let sys = &hub.system;
    out.write_str("{\"fw\":")?;
    json::string(out, &sys.firmware)?;
    out.write_str(",\"chip\":")?;
    json::string(out, &sys.chip)?;
    write!(out, ",\"elo\":{}", sys.elo)?;
    out.write_str(",\"model\":")?;
    json::string(out, &sys.model)?;
    out.write_str(",\"arch\":")?;
    json::string(out, &sys.model_arch)?;
    write!(out, ",\"bytes\":{},\"temp\":", sys.model_bytes)?;
    match sys.temp_dc {
        Some(t) => {
            let sign = if t < 0 { "-" } else { "" };
            write!(
                out,
                "{sign}{}.{}",
                t.unsigned_abs() / 10,
                t.unsigned_abs() % 10
            )?;
        }
        None => out.write_str("null")?,
    }
    write!(
        out,
        ",\"hashKb\":{},\"hashPsram\":{},\"uptime\":{}}}",
        sys.hash_kb,
        sys.hash_psram,
        now / 1000
    )
}

/// Network status: `{"mode":"ap","ap":"ESP-Chess-1A2B","sta":"","ip":"","err":"","connecting":false,"owner":true}`
fn write_net<W: Write>(hub: &Hub, owner: bool, out: &mut W) -> fmt::Result {
    let net = &hub.net;
    out.write_str("{\"mode\":")?;
    json::string(out, &net.mode)?;
    out.write_str(",\"ap\":")?;
    json::string(out, &net.ap_ssid)?;
    out.write_str(",\"sta\":")?;
    json::string(out, &net.sta_ssid)?;
    out.write_str(",\"ip\":")?;
    json::string(out, &net.sta_ip)?;
    out.write_str(",\"err\":")?;
    json::string(out, &net.sta_error)?;
    write!(
        out,
        ",\"connecting\":{},\"owner\":{owner}}}",
        net.connecting
    )
}

/// Game state. `since` = number of moves the web app already has (only newer ones are sent).
pub fn write_game<W: Write>(
    hub: &Hub,
    game: &Game,
    now: u64,
    me: Option<PlayerId>,
    since: usize,
    out: &mut W,
) -> fmt::Result {
    let since = since.min(game.moves.len());
    let until = (since + MOVES_PER_RESPONSE).min(game.moves.len());

    write!(
        out,
        "{{\"ok\":true,\"id\":{},\"v\":{},\"demo\":{},\"w\":",
        game.id, game.version, game.demo
    )?;
    write_participant(hub, game.players[0], now, out)?;
    out.write_str(",\"b\":")?;
    write_participant(hub, game.players[1], now, out)?;

    out.write_str(",\"you\":")?;
    match me.and_then(|id| game.color_of(id)) {
        Some(color) => write!(out, "\"{}\"", color_code(color))?,
        None => out.write_str("null")?,
    }

    write!(
        out,
        ",\"ply\":{},\"from\":{since},\"more\":{},\"moves\":[",
        game.ply(),
        until < game.moves.len()
    )?;
    for (i, mv) in game.moves[since..until].iter().enumerate() {
        if i > 0 {
            out.write_char(',')?;
        }
        write!(out, "\"{mv}\"")?;
    }
    out.write_str("],\"san\":[")?;
    let mut result = Ok(());
    game.san_list(since, |index, san| {
        if index < until && result.is_ok() {
            if index > since {
                result = out.write_char(',');
            }
            if result.is_ok() {
                result = write!(out, "\"{san}\"");
            }
        }
    });
    result?;

    out.write_str("],\"fen\":\"")?;
    game.position.write_fen(out)?;
    write!(
        out,
        "\",\"turn\":\"{}\",\"check\":{},",
        color_code(game.side_to_move()),
        game.position.in_check()
    )?;
    write_status(game, out)?;

    // Legal moves in the current position (the web app draws move dots and detects promotions).
    out.write_str(",\"legal\":[")?;
    if game.is_active() {
        for (i, mv) in legal_moves(&game.position).iter().enumerate() {
            if i > 0 {
                out.write_char(',')?;
            }
            write!(out, "\"{mv}\"")?;
        }
    }
    out.write_char(']')?;

    out.write_str(",\"clock\":")?;
    match game.clock {
        Some(clock) => {
            let side = game.side_to_move();
            write!(
                out,
                "{{\"w\":{},\"b\":{},\"running\":{},\"inc\":{},\"base\":{}}}",
                clock.remaining(Color::White, side, now),
                clock.remaining(Color::Black, side, now),
                clock.running_since.is_some() && game.is_active(),
                clock.control.increment_ms,
                clock.control.base_ms
            )?;
        }
        None => out.write_str("null")?,
    }

    let offer = |o: Option<Color>| o.map(color_code);
    out.write_str(",\"draw\":")?;
    json::opt_str(out, offer(game.draw_offer))?;
    out.write_str(",\"rematch\":")?;
    json::opt_str(out, offer(game.rematch_offer))?;
    write!(
        out,
        ",\"thinking\":{},\"hint\":",
        hub.is_engine_thinking_on(game.id)
    )?;
    match game.hint {
        Some((ply, mv)) if ply == game.ply() => write!(out, "\"{mv}\"")?,
        _ => out.write_str("null")?,
    }
    let engine = &hub.engine;
    let thinking_here = hub.is_engine_thinking_on(game.id);
    write!(
        out,
        ",\"depth\":{},\"nps\":{},\"viewers\":{}}}",
        if thinking_here { engine.depth } else { 0 },
        if thinking_here { engine.nps } else { 0 },
        hub.spectators(game, now)
    )
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::String as StdString;

    use super::*;

    fn call(hub: &mut Hub, now: u64, path: &str, query: &str) -> (ApiOutcome, StdString) {
        let mut out = StdString::new();
        let outcome = handle(hub, now, path, query, &mut out).unwrap();
        (outcome, out)
    }

    fn token_of(json: &str) -> StdString {
        let start = json.find("\"token\":\"").unwrap() + 9;
        json[start..start + 16].into()
    }

    #[test]
    fn full_lobby_fits_json_buffer() {
        // Worst case: all player slots with the longest nicknames, games, seeks and a filled
        // SystemInfo – the lobby response must fit the web server's buffer (chess-web JSON_BUFFER).
        use crate::hub::{ColorChoice, MAX_PLAYERS};
        let mut hub = Hub::new(9);
        hub.tick(0);
        let _ = hub.system.firmware.push_str("0.2.0 (8f37cbd-dirty)");
        let _ = hub.system.chip.push_str("ESP32-C6, 160 MHz");
        hub.system.elo = 2550;
        let _ = hub.system.model.push_str("h128-v4-ob8-i8");
        let _ = hub.system.model_arch.push_str("768→128×2→1, i8, 8 buckets");
        hub.system.model_bytes = 104_000;
        hub.system.temp_dc = Some(-123);
        hub.system.hash_kb = 1792;
        hub.system.hash_psram = true;
        let mut first_token = None;
        for i in 0..MAX_PLAYERS {
            // 16 characters, mostly 2-byte UTF-8 = the longest possible nickname
            let nick = std::format!("Øehoø{i:02}ëöüäåñçõæ");
            if let Ok((id, token)) = hub.hello(0, None, &nick) {
                first_token.get_or_insert(token);
                if i % 3 == 0 {
                    let _ = hub.play_engine(0, id, 5, ColorChoice::White, None);
                } else {
                    let _ = hub.create_seek(0, id, None, ColorChoice::Random);
                }
            }
        }
        let t = std::format!("t={:016x}", first_token.unwrap());
        let (_, lobby) = call(&mut hub, 0, "/api/lobby", &t);
        // The web server's JSON_BUFFER is 5120 B – keep a ~10 % margin
        assert!(lobby.len() < 5120 - 512, "lobby is {} B", lobby.len());
        assert!(lobby.contains(
            "\"sys\":{\"fw\":\"0.2.0 (8f37cbd-dirty)\",\"chip\":\"ESP32-C6, 160 MHz\",\"elo\":2550,"
        ));
        assert!(lobby.contains(
            "\"bytes\":104000,\"temp\":-12.3,\"hashKb\":1792,\"hashPsram\":true,\"uptime\":"
        ));
    }

    #[test]
    fn full_flow_between_two_players() {
        let mut hub = Hub::new(11);
        hub.set_demo_enabled(false);
        let (_, alice) = call(&mut hub, 0, "/api/hello", "nick=Alice");
        assert!(alice.contains("\"ok\":true"), "{alice}");
        let ta = token_of(&alice);
        let (_, bob) = call(&mut hub, 0, "/api/hello", "nick=Bob");
        let tb = token_of(&bob);

        let (_, r) = call(
            &mut hub,
            0,
            "/api/seek",
            &std::format!("t={ta}&min=5&inc=3&color=w"),
        );
        assert_eq!(r, "{\"ok\":true}");
        let (_, lobby) = call(&mut hub, 0, "/api/lobby", &std::format!("t={tb}"));
        assert!(lobby.contains("\"seeks\":[{\"id\":1"), "{lobby}");
        let (_, r) = call(&mut hub, 0, "/api/accept", &std::format!("t={tb}&seek=1"));
        assert!(r.contains("\"game\":1"), "{r}");

        let (_, r) = call(
            &mut hub,
            0,
            "/api/move",
            &std::format!("t={ta}&id=1&m=e2e4"),
        );
        assert_eq!(r, "{\"ok\":true}");
        let (_, r) = call(
            &mut hub,
            0,
            "/api/move",
            &std::format!("t={ta}&id=1&m=d2d4"),
        );
        assert!(r.contains("not_your_turn"));

        let (_, game) = call(&mut hub, 0, "/api/game", &std::format!("id=1&t={tb}"));
        assert!(game.contains("\"you\":\"b\""), "{game}");
        assert!(game.contains("\"moves\":[\"e2e4\"]"), "{game}");
        assert!(game.contains("\"san\":[\"e4\"]"), "{game}");
        assert!(game.contains("\"legal\":[\""), "{game}");
        assert!(game.contains("\"clock\":{\"w\":300000"), "{game}");

        // Only new moves.
        let (_, game) = call(&mut hub, 0, "/api/game", "id=1&since=1");
        assert!(game.contains("\"moves\":[]"), "{game}");
        assert!(game.contains("\"you\":null"));
    }

    #[test]
    fn logout_frees_the_session() {
        let mut hub = Hub::new(12);
        hub.set_demo_enabled(false);
        let (_, hello) = call(&mut hub, 0, "/api/hello", "nick=Leaving");
        let t = token_of(&hello);
        call(&mut hub, 0, "/api/engine", &std::format!("t={t}&level=1"));
        let (_, r) = call(&mut hub, 0, "/api/bye", &std::format!("t={t}"));
        assert_eq!(r, "{\"ok\":true}");
        assert!(hub.players().is_empty());
        assert!(!hub.game(1).unwrap().is_active());
        // The nickname is free again.
        let (_, again) = call(&mut hub, 0, "/api/hello", "nick=Leaving");
        assert!(again.contains("\"ok\":true"));
    }

    #[test]
    fn unknown_paths_and_errors() {
        let mut hub = Hub::new(1);
        assert_eq!(call(&mut hub, 0, "/api/nope", "").0, ApiOutcome::NotFound);
        let (_, r) = call(&mut hub, 0, "/api/move", "t=0000000000000001&id=1&m=e2e4");
        assert!(r.contains("no_session"));
        let (_, r) = call(&mut hub, 0, "/api/hello", "nick=%3Cb%3E");
        assert!(r.contains("bad_nick"));
    }

    #[test]
    fn engine_game_via_api() {
        let mut hub = Hub::new(2);
        hub.set_demo_enabled(false);
        let (_, hello) = call(&mut hub, 0, "/api/hello", "nick=Eva");
        let t = token_of(&hello);
        let (_, r) = call(
            &mut hub,
            0,
            "/api/engine",
            &std::format!("t={t}&level=4&color=b"),
        );
        assert!(r.contains("\"game\":1"), "{r}");
        let (_, game) = call(&mut hub, 0, "/api/game", &std::format!("id=1&t={t}"));
        assert!(
            game.contains("\"w\":{\"name\":null,\"engine\":true,\"level\":4}"),
            "{game}"
        );
        // An invalid time control is an error.
        let (_, r) = call(&mut hub, 0, "/api/engine", &std::format!("t={t}&min=7"));
        assert!(r.contains("bad_request") || r.contains("already_playing"));
    }

    #[test]
    fn wifi_only_for_owner() {
        let mut hub = Hub::new(3);
        hub.set_provisioning(true);
        let (_, owner) = call(&mut hub, 0, "/api/hello", "nick=Owner");
        let to = token_of(&owner);
        assert!(owner.contains("\"owner\":true"));
        let (_, guest) = call(&mut hub, 0, "/api/hello", "nick=Guest");
        let tg = token_of(&guest);

        let (outcome, r) = call(
            &mut hub,
            0,
            "/api/wifi",
            &std::format!("t={tg}&ssid=Home&pass=secret123"),
        );
        assert_eq!(outcome, ApiOutcome::Json);
        assert!(r.contains("not_allowed"));

        let (outcome, _) = call(
            &mut hub,
            0,
            "/api/wifi",
            &std::format!("t={to}&ssid=Home%20Net&pass=secret123"),
        );
        match outcome {
            ApiOutcome::WifiSave { ssid, password } => {
                assert_eq!(ssid.as_str(), "Home Net");
                assert_eq!(password.as_str(), "secret123");
            }
            other => panic!("expected WifiSave, got {other:?}"),
        }
        // Short password (WPA needs ≥ 8 characters).
        let (_, r) = call(
            &mut hub,
            0,
            "/api/wifi",
            &std::format!("t={to}&ssid=Home&pass=short"),
        );
        assert!(r.contains("bad_request"));
    }

    #[test]
    fn long_game_is_paged() {
        let mut hub = Hub::new(4);
        hub.set_demo_enabled(false);
        let (a, _) = hub.hello(0, None, "Alice").unwrap();
        let (b, _) = hub.hello(0, None, "Bob").unwrap();
        let seek = hub.create_seek(0, a, None, ColorChoice::White).unwrap();
        let id = hub.accept_seek(0, b, seek).unwrap();
        // Knights back and forth plus pawn pushes, to avoid threefold repetition.
        let mut plies = 0;
        'outer: for pawn in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            for (who, mv) in [
                (a, std::format!("{pawn}2{pawn}4")),
                (b, std::format!("{pawn}7{pawn}5")),
                (a, "g1h3".into()),
                (b, "g8h6".into()),
                (a, "h3g1".into()),
                (b, "h6g8".into()),
            ] {
                if hub.make_move(0, who, id, &mv).is_err() {
                    break 'outer;
                }
                plies += 1;
            }
        }
        assert!(plies >= 40);
        let mut out = StdString::new();
        let game = hub.game(id).unwrap();
        write_game(&hub, game, 0, None, 0, &mut out).unwrap();
        assert!(out.contains("\"more\":false"));
        assert!(out.len() < 4_000, "response is {} B", out.len());
    }
}
