//! URL query parameter parsing (`?t=abc&id=12&nick=Zo%C3%AB%20M%C3%BCller`).
//!
//! The web app sends all data as URL parameters (even for POST requests),
//! so no JSON parser is needed. Values are percent-encoded.

use heapless::String;

/// Finds parameter `key` in the query string and returns its raw (still encoded) value.
pub fn raw<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key).then_some(value)
    })
}

/// Parameter decoded into a string with capacity `N` bytes (longer value → `None`).
pub fn text<const N: usize>(query: &str, key: &str) -> Option<String<N>> {
    decode(raw(query, key)?)
}

/// Numeric parameter.
pub fn number<T: core::str::FromStr>(query: &str, key: &str) -> Option<T> {
    raw(query, key)?.parse().ok()
}

/// Session token (16 hex digits).
pub fn token(query: &str) -> Option<u64> {
    u64::from_str_radix(raw(query, "t")?, 16).ok()
}

/// Decodes percent-encoding (`%C3%BC` → "ü", `+` → space). Invalid UTF-8 → `None`.
pub fn decode<const N: usize>(value: &str) -> Option<String<N>> {
    let mut bytes = heapless::Vec::<u8, N>::new();
    let input = value.as_bytes();
    let mut i = 0;
    while i < input.len() {
        let byte = match input[i] {
            b'+' => b' ',
            b'%' => {
                // `%` must be followed by exactly two hex digits.
                let hex = input.get(i + 1..i + 3)?;
                let hex = core::str::from_utf8(hex).ok()?;
                i += 2;
                u8::from_str_radix(hex, 16).ok()?
            }
            other => other,
        };
        bytes.push(byte).ok()?;
        i += 1;
    }
    let text = core::str::from_utf8(&bytes).ok()?;
    String::try_from(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_parameters() {
        let q = "t=00ff&id=12&nick=Zo%C3%AB%20M%C3%BCller&flag";
        assert_eq!(raw(q, "id"), Some("12"));
        assert_eq!(raw(q, "flag"), Some(""));
        assert_eq!(raw(q, "missing"), None);
        assert_eq!(number::<u32>(q, "id"), Some(12));
        assert_eq!(token(q), Some(0xff));
        let nick: String<32> = text(q, "nick").unwrap();
        assert_eq!(nick.as_str(), "Zoë Müller");
    }

    #[test]
    fn rejects_bad_encoding() {
        assert!(decode::<16>("%ZZ").is_none());
        assert!(decode::<16>("%C3").is_none()); // incomplete UTF-8
        assert!(decode::<4>("toolong").is_none()); // capacity overflow
        assert_eq!(decode::<16>("a+b").unwrap().as_str(), "a b");
        assert!(decode::<16>("%4").is_none());
    }
}
