//! State snapshot for the board's display.
//!
//! The display is drawn outside the hub lock, so we take one small copy of
//! everything it needs (positions, names, stats) and release the hub at once.

use chess_core::{Move, Position};
use heapless::{String, Vec};

use crate::game::{Game, GameId, Participant};
use crate::hub::{EngineStats, Hub};

/// How many games fit on the overview screen.
pub const MAX_PREVIEWS: usize = 4;

/// Short player name for the display (engine = "Engine 5", demo = "Engine").
pub type ShortName = String<20>;

/// Preview of one game.
#[derive(Clone, Debug)]
pub struct GamePreview {
    pub id: GameId,
    pub version: u32,
    pub position: Position,
    pub last_move: Option<Move>,
    pub white: ShortName,
    pub black: ShortName,
    pub demo: bool,
    pub active: bool,
    /// Result ("1-0", "0-1", "1/2-1/2", "*").
    pub result: &'static str,
    pub ply: u16,
}

/// Everything the display needs.
#[derive(Clone, Debug)]
pub struct DisplayView {
    /// Main game on the display (a human game, otherwise the demo).
    pub featured: Option<GamePreview>,
    /// Other running games for the overview.
    pub previews: Vec<GamePreview, MAX_PREVIEWS>,
    pub engine: EngineStats,
    pub players_online: u8,
    pub active_games: u8,
    /// Number of running human games (excluding the demo).
    pub human_games: u8,
    /// Peak number of concurrent online players since boot.
    pub peak_online: u16,
    /// Human games started since boot.
    pub games_started: u32,
}

fn short_name(hub: &Hub, participant: Participant, demo: bool) -> ShortName {
    let mut name = ShortName::new();
    match participant {
        Participant::Human(_) => {
            let nick = hub.participant_nick(participant).unwrap_or("?");
            for c in nick.chars() {
                if name.push(c).is_err() {
                    break;
                }
            }
        }
        Participant::Engine { level } => {
            let _ = if demo {
                core::fmt::Write::write_str(&mut name, "Engine")
            } else {
                core::fmt::Write::write_fmt(&mut name, format_args!("Engine {level}"))
            };
        }
    }
    name
}

fn preview(hub: &Hub, game: &Game) -> GamePreview {
    GamePreview {
        id: game.id,
        version: game.version,
        position: game.position.clone(),
        last_move: game.last_move(),
        white: short_name(hub, game.players[0], game.demo),
        black: short_name(hub, game.players[1], game.demo),
        demo: game.demo,
        active: game.is_active(),
        result: game.outcome.map_or("*", |o| o.result_text()),
        ply: game.ply(),
    }
}

impl DisplayView {
    /// Takes a snapshot. `rotation` selects which human game to show (the
    /// display increments it every few seconds when several games run).
    /// `pinned = Some(i)` selects the i-th running game instead (including the
    /// demo, in [`DisplayView::active_game_count`] order) – button paging.
    pub fn capture(hub: &Hub, now: u64, rotation: usize, pinned: Option<usize>) -> DisplayView {
        let games = hub.games();
        let human_active = || games.iter().filter(|g| g.is_active() && !g.demo);
        let human_count = human_active().count();

        let pinned_game = pinned.and_then(|i| games.iter().filter(|g| g.is_active()).nth(i));
        let featured = if pinned_game.is_some() {
            pinned_game
        } else if human_count > 0 {
            human_active().nth(rotation % human_count)
        } else {
            hub.demo_game()
                .or_else(|| games.iter().max_by_key(|g| g.last_activity_ms))
        };

        let mut previews = Vec::new();
        for game in games.iter().filter(|g| g.is_active()).take(MAX_PREVIEWS) {
            let _ = previews.push(preview(hub, game));
        }

        DisplayView {
            featured: featured.map(|g| preview(hub, g)),
            previews,
            engine: hub.engine,
            players_online: hub.online_count(now).min(255) as u8,
            active_games: games.iter().filter(|g| g.is_active()).count() as u8,
            human_games: human_count as u8,
            peak_online: hub.peak_online,
            games_started: hub.games_started,
        }
    }

    /// Number of running games (for button paging, see `pinned` in [`DisplayView::capture`]).
    pub fn active_game_count(hub: &Hub) -> usize {
        hub.games().iter().filter(|g| g.is_active()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::ColorChoice;

    #[test]
    fn featured_prefers_human_games() {
        let mut hub = Hub::new(1);
        hub.tick(0);
        let view = DisplayView::capture(&hub, 0, 0, None);
        assert!(view.featured.as_ref().unwrap().demo);
        assert_eq!(view.featured.as_ref().unwrap().white.as_str(), "Engine");

        let (alice, _) = hub.hello(0, None, "Alice").unwrap();
        hub.play_engine(0, alice, 5, ColorChoice::White, None)
            .unwrap();
        let view = DisplayView::capture(&hub, 0, 0, None);
        let featured = view.featured.unwrap();
        assert!(!featured.demo);
        assert_eq!(featured.white.as_str(), "Alice");
        assert_eq!(featured.black.as_str(), "Engine 5");
        assert_eq!(view.players_online, 1);
        assert_eq!(view.human_games, 1);
        assert_eq!(view.previews.len(), 2);
    }
}
