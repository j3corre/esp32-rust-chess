//! Games: players, moves, clocks, game termination.

use chess_core::{Color, GameStatus, Move, Position, SanText, game_status, legal_moves, to_san};
use heapless::Vec;

use crate::player::PlayerId;

/// Game identifier (increasing; wraps around to 1 on overflow).
pub type GameId = u16;

/// Maximum game length in plies; the game is then drawn
/// (saves RAM; rarely reached in practice).
pub const MAX_PLIES: usize = 400;

/// Who plays one side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Participant {
    Human(PlayerId),
    /// Engine at level 1–8; level 0 = demo (10 s per move).
    Engine {
        level: u8,
    },
}

impl Participant {
    pub fn is_engine(self) -> bool {
        matches!(self, Participant::Engine { .. })
    }

    pub fn human(self) -> Option<PlayerId> {
        match self {
            Participant::Human(id) => Some(id),
            Participant::Engine { .. } => None,
        }
    }
}

/// Time control: base time + increment per move (Fischer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeControl {
    pub base_ms: u64,
    pub increment_ms: u64,
}

impl TimeControl {
    /// From minutes and seconds ("5+3").
    pub const fn new(minutes: u64, increment_s: u64) -> TimeControl {
        TimeControl {
            base_ms: minutes * 60_000,
            increment_ms: increment_s * 1000,
        }
    }

    /// Allowed time controls (offered by the web app); others are rejected.
    pub const ALLOWED: [(u64, u64); 7] =
        [(1, 0), (3, 2), (5, 0), (5, 3), (10, 0), (15, 10), (30, 0)];

    pub fn from_minutes(minutes: u64, increment_s: u64) -> Option<TimeControl> {
        TimeControl::ALLOWED
            .contains(&(minutes, increment_s))
            .then(|| TimeControl::new(minutes, increment_s))
    }
}

/// Chess clock.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub control: TimeControl,
    /// Remaining time [white, black] as of `running_since`.
    pub remaining_ms: [u64; 2],
    /// Since when the side to move has been running (`None` = stopped, until the 2nd ply).
    pub running_since: Option<u64>,
}

impl Clock {
    pub fn new(control: TimeControl) -> Clock {
        Clock {
            control,
            remaining_ms: [control.base_ms; 2],
            running_since: None,
        }
    }

    /// Remaining time of `color` at `now` (accounts for a running clock).
    pub fn remaining(&self, color: Color, to_move: Color, now: u64) -> u64 {
        let base = self.remaining_ms[color.index()];
        match self.running_since {
            Some(since) if color == to_move => base.saturating_sub(now.saturating_sub(since)),
            _ => base,
        }
    }
}

/// How a game ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Checkmate,
    Stalemate,
    Repetition,
    FiftyMoves,
    InsufficientMaterial,
    Resignation,
    Timeout,
    DrawAgreed,
    /// Game aborted before the 2nd ply.
    Aborted,
    /// A player has been silent for too long.
    Abandoned,
    /// Maximum game length exceeded.
    MaxLength,
}

impl EndReason {
    /// Code for the web app (translated in the browser).
    pub fn code(self) -> &'static str {
        match self {
            EndReason::Checkmate => "mate",
            EndReason::Stalemate => "stalemate",
            EndReason::Repetition => "repetition",
            EndReason::FiftyMoves => "fifty",
            EndReason::InsufficientMaterial => "material",
            EndReason::Resignation => "resign",
            EndReason::Timeout => "timeout",
            EndReason::DrawAgreed => "agreed",
            EndReason::Aborted => "aborted",
            EndReason::Abandoned => "abandoned",
            EndReason::MaxLength => "maxlength",
        }
    }
}

/// Result of a finished game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Winner; `None` = draw (or aborted game).
    pub winner: Option<Color>,
    pub reason: EndReason,
}

impl Outcome {
    /// PGN result: "1-0", "0-1", "1/2-1/2", "*" (aborted).
    pub fn result_text(&self) -> &'static str {
        match (self.winner, self.reason) {
            (_, EndReason::Aborted) => "*",
            (Some(Color::White), _) => "1-0",
            (Some(Color::Black), _) => "0-1",
            (None, _) => "1/2-1/2",
        }
    }
}

/// Errors of in-game actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveError {
    GameOver,
    NotYourTurn,
    Illegal,
}

/// A single game.
#[derive(Clone, Debug)]
pub struct Game {
    pub id: GameId,
    /// [white, black]
    pub players: [Participant; 2],
    pub position: Position,
    pub moves: Vec<Move, MAX_PLIES>,
    pub clock: Option<Clock>,
    pub outcome: Option<Outcome>,
    /// Who offers a draw (cleared by the next move).
    pub draw_offer: Option<Color>,
    /// Who offers a rematch (after the game ended).
    pub rematch_offer: Option<Color>,
    /// Incremented on every change – tells the web app to redraw.
    pub version: u32,
    pub created_ms: u64,
    pub last_activity_ms: u64,
    /// Engine-vs-engine demo game (runs while nobody plays the engine).
    pub demo: bool,
    /// Seed for engine noise (different game = different mistakes at low levels).
    pub seed: u64,
    /// Latest hint (move) for the human and the ply it belongs to.
    pub hint: Option<(u16, Move)>,
}

impl Game {
    pub fn new(
        id: GameId,
        white: Participant,
        black: Participant,
        clock: Option<TimeControl>,
        now: u64,
        seed: u64,
    ) -> Game {
        Game {
            id,
            players: [white, black],
            position: Position::start(),
            moves: Vec::new(),
            clock: clock.map(Clock::new),
            outcome: None,
            draw_offer: None,
            rematch_offer: None,
            version: 1,
            created_ms: now,
            last_activity_ms: now,
            demo: false,
            seed,
            hint: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.outcome.is_none()
    }

    pub fn ply(&self) -> u16 {
        self.moves.len() as u16
    }

    pub fn side_to_move(&self) -> Color {
        self.position.side_to_move()
    }

    /// Who is to move.
    pub fn to_move(&self) -> Participant {
        self.players[self.side_to_move().index()]
    }

    /// Which colour the player plays (or `None` if not in this game).
    pub fn color_of(&self, player: PlayerId) -> Option<Color> {
        Color::ALL
            .into_iter()
            .find(|c| self.players[c.index()] == Participant::Human(player))
    }

    pub fn has_human(&self, player: PlayerId) -> bool {
        self.color_of(player).is_some()
    }

    pub fn is_vs_engine(&self) -> bool {
        self.players.iter().any(|p| p.is_engine()) && !self.demo
    }

    pub fn last_move(&self) -> Option<Move> {
        self.moves.last().copied()
    }

    fn touch(&mut self, now: u64) {
        self.version = self.version.wrapping_add(1);
        self.last_activity_ms = now;
    }

    /// Hashes of all positions before the current one (for repetition detection).
    /// Computed by replaying the moves – no need to keep them in RAM.
    pub fn history_hashes<const N: usize>(&self) -> Vec<u64, N> {
        let mut hashes = Vec::new();
        let mut pos = Position::start();
        // Positions since the last irreversible move suffice (max. 100 plies).
        let skip = self.moves.len().saturating_sub(N);
        for (i, &mv) in self.moves.iter().enumerate() {
            if i >= skip {
                let _ = hashes.push(pos.hash());
            }
            pos = pos.make_move(mv);
        }
        hashes
    }

    /// Position after `ply` plies (by replaying moves).
    pub fn position_at(&self, ply: usize) -> Position {
        let mut pos = Position::start();
        for &mv in self.moves.iter().take(ply) {
            pos = pos.make_move(mv);
        }
        pos
    }

    /// SAN of every move from `from` on (replays the game – only used for web responses).
    pub fn san_list<F: FnMut(usize, SanText)>(&self, from: usize, mut each: F) {
        let mut pos = Position::start();
        for (i, &mv) in self.moves.iter().enumerate() {
            if i >= from {
                each(i, to_san(&pos, mv));
            }
            pos = pos.make_move(mv);
        }
    }

    /// Checks the clock: if the side to move ran out of time, the game ends.
    /// Returns `true` if the game has just ended.
    pub fn check_flag(&mut self, now: u64) -> bool {
        if !self.is_active() {
            return false;
        }
        let Some(clock) = self.clock else {
            return false;
        };
        let side = self.side_to_move();
        if clock.remaining(side, side, now) > 0 {
            return false;
        }
        // A timeout is a draw if the opponent has no mating material.
        let winner = side.opponent();
        let can_mate = has_mating_material(&self.position, winner);
        self.finish(
            now,
            Outcome {
                winner: can_mate.then_some(winner),
                reason: EndReason::Timeout,
            },
        );
        true
    }

    /// Plays a move (UCI already parsed into `Move`) for the side to move.
    pub fn play(&mut self, mv: Move, now: u64) -> Result<(), MoveError> {
        if !self.is_active() {
            return Err(MoveError::GameOver);
        }
        if self.check_flag(now) {
            return Err(MoveError::GameOver);
        }
        if !legal_moves(&self.position).contains(mv) {
            return Err(MoveError::Illegal);
        }
        let side = self.side_to_move();

        // Clock: subtract time used, add increment, start the opponent's clock.
        if let Some(clock) = self.clock.as_mut() {
            if let Some(since) = clock.running_since {
                let used = now.saturating_sub(since);
                let left = clock.remaining_ms[side.index()].saturating_sub(used);
                clock.remaining_ms[side.index()] = left + clock.control.increment_ms;
            }
            // The clock starts only after both sides' first move (like lichess).
            if self.moves.len() + 1 >= 2 {
                clock.running_since = Some(now);
            }
        }

        let _ = self.moves.push(mv);
        self.position = self.position.make_move(mv);
        self.draw_offer = None;
        self.hint = None;
        self.touch(now);
        self.update_status(now);
        Ok(())
    }

    /// Checks whether the game ended after a move (mate, stalemate, draws, length).
    fn update_status(&mut self, now: u64) {
        let history: Vec<u64, 128> = self.history_hashes();
        let outcome = match game_status(&self.position, &history) {
            GameStatus::Ongoing => {
                if self.moves.len() >= MAX_PLIES {
                    Some(Outcome {
                        winner: None,
                        reason: EndReason::MaxLength,
                    })
                } else {
                    None
                }
            }
            GameStatus::Checkmate { winner } => Some(Outcome {
                winner: Some(winner),
                reason: EndReason::Checkmate,
            }),
            GameStatus::Stalemate => Some(draw(EndReason::Stalemate)),
            GameStatus::Repetition => Some(draw(EndReason::Repetition)),
            GameStatus::FiftyMoves => Some(draw(EndReason::FiftyMoves)),
            GameStatus::InsufficientMaterial => Some(draw(EndReason::InsufficientMaterial)),
        };
        if let Some(outcome) = outcome {
            self.finish(now, outcome);
        }
    }

    /// Ends the game (and stops the clock).
    pub fn finish(&mut self, now: u64, outcome: Outcome) {
        if let Some(clock) = self.clock.as_mut() {
            if let Some(since) = clock.running_since {
                let side = self.position.side_to_move();
                let used = now.saturating_sub(since);
                clock.remaining_ms[side.index()] =
                    clock.remaining_ms[side.index()].saturating_sub(used);
            }
            clock.running_since = None;
        }
        self.outcome = Some(outcome);
        self.draw_offer = None;
        self.touch(now);
    }

    /// Resignation / abort by the player of `color`.
    pub fn resign(&mut self, color: Color, now: u64) {
        if !self.is_active() {
            return;
        }
        let outcome = if self.moves.len() < 2 {
            Outcome {
                winner: None,
                reason: EndReason::Aborted,
            }
        } else {
            Outcome {
                winner: Some(color.opponent()),
                reason: EndReason::Resignation,
            }
        };
        self.finish(now, outcome);
    }

    /// Takes back moves so the human playing `color` is to move again
    /// (engine games only). Returns `false` if there is nothing to take back.
    pub fn take_back(&mut self, color: Color, now: u64) -> bool {
        if !self.is_active() || !self.is_vs_engine() {
            return false;
        }
        // Plies to remove: if the human is to move, undo both their move and the engine reply.
        let remove = if self.side_to_move() == color { 2 } else { 1 };
        if self.moves.len() < remove {
            return false;
        }
        for _ in 0..remove {
            self.moves.pop();
        }
        self.position = self.position_at(self.moves.len());
        if let Some(clock) = self.clock.as_mut() {
            clock.running_since = if self.moves.len() >= 2 {
                Some(now)
            } else {
                None
            };
        }
        self.hint = None;
        self.draw_offer = None;
        self.touch(now);
        true
    }

    /// Draw offer. If the opponent already offered one, the draw is accepted.
    /// Returns `true` if the game ended in a draw.
    pub fn offer_draw(&mut self, color: Color, now: u64) -> bool {
        if !self.is_active() {
            return false;
        }
        match self.draw_offer {
            Some(offer) if offer != color => {
                self.finish(now, draw(EndReason::DrawAgreed));
                true
            }
            _ => {
                self.draw_offer = Some(color);
                self.touch(now);
                false
            }
        }
    }

    pub fn decline_draw(&mut self, now: u64) {
        if self.draw_offer.take().is_some() {
            self.touch(now);
        }
    }

    /// Bumps the version (e.g. when the spectator count or hint changed).
    pub fn bump(&mut self, now: u64) {
        self.touch(now);
    }
}

fn draw(reason: EndReason) -> Outcome {
    Outcome {
        winner: None,
        reason,
    }
}

/// Does the side have enough material to possibly mate? (For timeouts.)
fn has_mating_material(pos: &Position, color: Color) -> bool {
    use chess_core::PieceKind::*;
    let pieces = |kind| pos.pieces(color, kind).count_ones();
    pieces(Pawn) > 0
        || pieces(Rook) > 0
        || pieces(Queen) > 0
        || pieces(Knight) + pieces(Bishop) >= 2
        || (pieces(Knight) + pieces(Bishop) == 1 && pos.color_bb(color.opponent()).count_ones() > 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::parse_uci;

    fn play(game: &mut Game, uci: &str, now: u64) -> Result<(), MoveError> {
        let mv = parse_uci(&game.position, uci).ok_or(MoveError::Illegal)?;
        game.play(mv, now)
    }

    fn human_game(clock: Option<TimeControl>) -> Game {
        Game::new(1, Participant::Human(1), Participant::Human(2), clock, 0, 7)
    }

    #[test]
    fn fools_mate_ends_game() {
        let mut game = human_game(None);
        for mv in ["f2f3", "e7e5", "g2g4"] {
            play(&mut game, mv, 0).unwrap();
        }
        play(&mut game, "d8h4", 0).unwrap();
        let outcome = game.outcome.unwrap();
        assert_eq!(outcome.winner, Some(Color::Black));
        assert_eq!(outcome.reason, EndReason::Checkmate);
        assert_eq!(outcome.result_text(), "0-1");
        assert_eq!(game.play(Move::NULL, 0), Err(MoveError::GameOver));
    }

    #[test]
    fn clock_starts_after_both_first_moves_and_flags() {
        let mut game = human_game(Some(TimeControl::new(1, 0)));
        play(&mut game, "e2e4", 5_000).unwrap();
        assert!(game.clock.unwrap().running_since.is_none());
        play(&mut game, "e7e5", 10_000).unwrap();
        // White's clock now runs from 10 s.
        assert_eq!(
            game.clock
                .unwrap()
                .remaining(Color::White, Color::White, 40_000),
            30_000
        );
        assert!(!game.check_flag(69_999));
        assert!(game.check_flag(70_000));
        let outcome = game.outcome.unwrap();
        assert_eq!(outcome.reason, EndReason::Timeout);
        assert_eq!(outcome.winner, Some(Color::Black));
    }

    #[test]
    fn increment_is_added() {
        let mut game = human_game(Some(TimeControl::new(3, 2)));
        play(&mut game, "e2e4", 0).unwrap();
        play(&mut game, "e7e5", 0).unwrap();
        play(&mut game, "g1f3", 10_000).unwrap(); // white used 10 s, +2 s
        assert_eq!(
            game.clock.unwrap().remaining_ms[0],
            180_000 - 10_000 + 2_000
        );
    }

    #[test]
    fn resign_before_second_ply_aborts() {
        let mut game = human_game(None);
        game.resign(Color::White, 0);
        assert_eq!(game.outcome.unwrap().reason, EndReason::Aborted);
        assert_eq!(game.outcome.unwrap().result_text(), "*");

        let mut game = human_game(None);
        play(&mut game, "e2e4", 0).unwrap();
        play(&mut game, "e7e5", 0).unwrap();
        game.resign(Color::Black, 0);
        assert_eq!(game.outcome.unwrap().winner, Some(Color::White));
    }

    #[test]
    fn draw_offer_and_accept() {
        let mut game = human_game(None);
        play(&mut game, "e2e4", 0).unwrap();
        assert!(!game.offer_draw(Color::Black, 0));
        assert_eq!(game.draw_offer, Some(Color::Black));
        // A move cancels the offer.
        play(&mut game, "e7e5", 0).unwrap();
        assert_eq!(game.draw_offer, None);
        assert!(!game.offer_draw(Color::White, 0));
        assert!(game.offer_draw(Color::Black, 0));
        assert_eq!(game.outcome.unwrap().reason, EndReason::DrawAgreed);
    }

    #[test]
    fn take_back_against_engine() {
        let mut game = Game::new(
            1,
            Participant::Human(1),
            Participant::Engine { level: 3 },
            None,
            0,
            1,
        );
        play(&mut game, "e2e4", 0).unwrap();
        play(&mut game, "e7e5", 0).unwrap();
        assert!(game.take_back(Color::White, 0));
        assert_eq!(game.ply(), 0);
        assert_eq!(game.position, Position::start());
        // No take-backs between humans.
        let mut human = human_game(None);
        play(&mut human, "e2e4", 0).unwrap();
        assert!(!human.take_back(Color::White, 0));
    }

    #[test]
    fn illegal_moves_are_rejected() {
        let mut game = human_game(None);
        assert_eq!(play(&mut game, "e2e5", 0), Err(MoveError::Illegal));
        assert_eq!(game.ply(), 0);
    }

    #[test]
    fn repetition_ends_game() {
        let mut game = human_game(None);
        for mv in [
            "g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8",
        ] {
            play(&mut game, mv, 0).unwrap();
        }
        assert_eq!(game.outcome.unwrap().reason, EndReason::Repetition);
    }

    #[test]
    fn allowed_time_controls() {
        assert!(TimeControl::from_minutes(5, 3).is_some());
        assert!(TimeControl::from_minutes(7, 7).is_none());
    }
}
