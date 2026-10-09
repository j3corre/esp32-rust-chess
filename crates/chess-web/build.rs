//! Build script – bundles the web app into a single file and gzips it.
//!
//! 1. Reads `web/index.html` and replaces `<!--INLINE:file-->` markers with the
//!    file contents (CSS into `<style>`, JS into `<script>`).
//! 2. Replaces the `<!--INLINE:pieces-->` marker with CSS classes `.wK`, `.bQ`, …
//!    holding the piece images (SVG as base64 data URIs).
//! 3. Gzips the result (≈ 4× smaller – faster loading over WiFi, less flash)
//!    and writes it to `OUT_DIR/index.html.gz`.
//!
//! The board and the simulator therefore serve a byte-for-byte identical file.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;

const PIECES: [&str; 12] = [
    "wK", "wQ", "wR", "wB", "wN", "wP", "bK", "bQ", "bR", "bB", "bN", "bP",
];

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let web = manifest.join("../../web");
    println!("cargo:rerun-if-changed={}", web.display());
    for entry in walk(&web) {
        println!("cargo:rerun-if-changed={}", entry.display());
    }

    let template = read(&web.join("index.html"));
    let mut html = String::with_capacity(template.len() * 4);
    let mut rest = template.as_str();
    while let Some(start) = rest.find("<!--INLINE:") {
        html.push_str(&rest[..start]);
        let after = &rest[start + "<!--INLINE:".len()..];
        let end = after.find("-->").expect("unterminated INLINE marker");
        let name = after[..end].trim();
        html.push_str(&inline(&web, name));
        rest = &after[end + 3..];
    }
    html.push_str(rest);

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out.join("index.html"), &html).unwrap();

    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(html.as_bytes()).unwrap();
    let gz = encoder.finish().unwrap();
    fs::write(out.join("index.html.gz"), gz).unwrap();
}

/// Content that replaces the `<!--INLINE:name-->` marker.
fn inline(web: &Path, name: &str) -> String {
    if name == "pieces" {
        let mut css = String::from("<style>");
        for piece in PIECES {
            let svg = read(&web.join("pieces").join(format!("{piece}.svg")));
            css.push_str(&format!(
                ".{piece}{{background-image:url(\"data:image/svg+xml;base64,{}\")}}",
                base64(svg.trim().as_bytes())
            ));
        }
        css.push_str("</style>");
        return css;
    }
    let content = read(&web.join(name));
    if name.ends_with(".css") {
        format!("<style>\n{content}\n</style>")
    } else if name.ends_with(".js") {
        assert!(!content.contains("</script"), "{name} contains </script>");
        format!("<script>\n{content}\n</script>")
    } else {
        content
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// All files in the directory (recursively) – for change tracking.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Tests and node_modules are not part of the app.
            if name == "node_modules"
                || name == "tests"
                || name.starts_with('.')
                || name == "test-results"
            {
                continue;
            }
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
    }
    files
}

/// Simple base64 encoding (no dependencies).
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
