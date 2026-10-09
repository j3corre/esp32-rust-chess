//! Allocation-free JSON writing helpers (directly into `core::fmt::Write`).

use core::fmt::{self, Write};

/// Writes text as a JSON string (quoted and escaped).
pub fn string<W: Write>(out: &mut W, text: &str) -> fmt::Result {
    out.write_char('"')?;
    for c in text.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            '\r' => out.write_str("\\r")?,
            '\t' => out.write_str("\\t")?,
            // Other control characters as \u00XX; everything else (incl. non-ASCII) unchanged.
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            // Escape < and > so the JSON cannot be abused to inject HTML.
            '<' => out.write_str("\\u003c")?,
            '>' => out.write_str("\\u003e")?,
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}

/// Writes `"key":` (keys are our own constants, no escaping needed).
pub fn key<W: Write>(out: &mut W, name: &str) -> fmt::Result {
    write!(out, "\"{name}\":")
}

/// A string value or `null`.
pub fn opt_str<W: Write>(out: &mut W, value: Option<&str>) -> fmt::Result {
    match value {
        Some(text) => string(out, text),
        None => out.write_str("null"),
    }
}

/// Action response: `{"ok":true}` or `{"ok":false,"err":"code"}`.
pub fn result<W: Write>(out: &mut W, outcome: Result<(), &str>) -> fmt::Result {
    match outcome {
        Ok(()) => out.write_str("{\"ok\":true}"),
        Err(code) => write!(out, "{{\"ok\":false,\"err\":\"{code}\"}}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use heapless::String;

    #[test]
    fn escapes_special_characters() {
        let mut out: String<64> = String::new();
        string(&mut out, "a\"b\\c\n<ü>").unwrap();
        assert_eq!(out.as_str(), "\"a\\\"b\\\\c\\n\\u003cü\\u003e\"");
    }

    #[test]
    fn result_codes() {
        let mut out: String<64> = String::new();
        result(&mut out, Err("illegal")).unwrap();
        assert_eq!(out.as_str(), "{\"ok\":false,\"err\":\"illegal\"}");
    }
}
