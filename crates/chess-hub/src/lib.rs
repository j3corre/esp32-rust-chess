//! # chess-hub
//!
//! Game server without a network layer (`no_std`, no heap): nicknames and
//! sessions, challenges, human-vs-human and human-vs-engine games, chess
//! clocks, an engine job queue, a demo game and the JSON API for the web app.
//!
//! | Module   | Contents |
//! |----------|----------|
//! | `hub`    | overall server state and its operations |
//! | `game`   | games, clocks, game termination |
//! | `player` | players, nickname rules |
//! | `api`    | handling `/api/*` requests → JSON |
//! | `json`, `params` | JSON writing, URL parameter parsing |
//! | `view`   | state snapshot for the display |

#![no_std]

pub mod api;
pub mod game;
pub mod hub;
pub mod json;
pub mod params;
pub mod player;
pub mod stored;
pub mod view;

pub use api::{ApiOutcome, handle};
/// Length of the principal variation in [`EngineStats::pv`].
pub use chess_engine::PV_LEN;
pub use game::{Game, GameId, Participant, TimeControl};
pub use hub::{EngineJob, EngineStats, Hub, HubError, JobKind, NetInfo, SystemInfo};
pub use view::{DisplayView, GamePreview};
