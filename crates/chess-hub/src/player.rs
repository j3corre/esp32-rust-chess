//! Player (a browser session) and nickname rules.

use heapless::String;

use crate::game::GameId;

/// Maximum nickname length in characters.
pub const NICK_MAX_CHARS: usize = 16;
/// Nickname capacity in bytes (UTF-8: up to 3 bytes per character).
pub const NICK_BYTES: usize = 48;

pub type Nick = String<NICK_BYTES>;

/// Player identifier (small number, stable for the session lifetime).
pub type PlayerId = u16;

/// One connected player.
#[derive(Clone, Debug)]
pub struct Player {
    pub id: PlayerId,
    /// Secret session token (the browser sends it with every request).
    pub token: u64,
    pub nick: Nick,
    /// When we last received a request from the player (ms).
    pub last_seen_ms: u64,
    /// Game the player is currently watching (not playing) – for the spectator count.
    pub watching: Option<GameId>,
}

impl Player {
    /// Is the player online (seen within the last `window_ms`)?
    pub fn is_online(&self, now_ms: u64, window_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_seen_ms) <= window_ms
    }
}

/// Nickname validation error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NickError {
    TooShort,
    TooLong,
    BadCharacter,
}

/// Validates and normalises a nickname: trims whitespace, checks length and characters.
/// Allowed are letters (including accented ones), digits, space, `_`, `-`, `.`.
pub fn validate_nick(raw: &str) -> Result<Nick, NickError> {
    let trimmed = raw.trim();
    let chars = trimmed.chars().count();
    if chars < 2 {
        return Err(NickError::TooShort);
    }
    if chars > NICK_MAX_CHARS {
        return Err(NickError::TooLong);
    }
    let mut nick = Nick::new();
    let mut previous_space = false;
    for c in trimmed.chars() {
        let allowed = c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.');
        if !allowed {
            return Err(NickError::BadCharacter);
        }
        // Collapse runs of spaces into one.
        if c == ' ' && previous_space {
            continue;
        }
        previous_space = c == ' ';
        nick.push(c).map_err(|_| NickError::TooLong)?;
    }
    Ok(nick)
}

/// Case-insensitive nickname comparison (for uniqueness).
pub fn same_nick(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nick_rules() {
        assert_eq!(
            validate_nick("  Zoë  Müller ").unwrap().as_str(),
            "Zoë Müller"
        );
        assert_eq!(validate_nick("a"), Err(NickError::TooShort));
        assert_eq!(validate_nick("abcdefghijklmnopq"), Err(NickError::TooLong));
        assert_eq!(validate_nick("<script>"), Err(NickError::BadCharacter));
        assert!(validate_nick("Ærøskøbing Zoë").is_ok());
        assert!(same_nick("Ölund", "öLUND"));
        assert!(!same_nick("Ann", "Anna"));
    }
}
