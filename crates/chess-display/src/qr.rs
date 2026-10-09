//! Heap-free QR code (qrcodegen-no-heap).

use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};

/// Highest QR version we allow (57×57 modules – enough for a URL or Wi-Fi login).
const MAX_VERSION: Version = Version::new(10);
const MAX_SIZE: usize = 57;
const BUFFER: usize = MAX_VERSION.buffer_len();

/// Encoded QR code – modules stored as bits (≈ 400 B).
pub struct Qr {
    bits: [u8; MAX_SIZE * MAX_SIZE / 8 + 1],
    size: i32,
}

impl Qr {
    /// Encodes text; `None` if the text is too long.
    pub fn encode(text: &str) -> Option<Qr> {
        let mut temp = [0u8; BUFFER];
        let mut out = [0u8; BUFFER];
        let code = QrCode::encode_text(
            text,
            &mut temp,
            &mut out,
            QrCodeEcc::Medium,
            Version::MIN,
            MAX_VERSION,
            None,
            true,
        )
        .ok()?;
        let size = code.size();
        let mut qr = Qr {
            bits: [0; MAX_SIZE * MAX_SIZE / 8 + 1],
            size,
        };
        for y in 0..size {
            for x in 0..size {
                if code.get_module(x, y) {
                    let i = (y * size + x) as usize;
                    qr.bits[i / 8] |= 1 << (i % 8);
                }
            }
        }
        Some(qr)
    }

    pub fn size(&self) -> i32 {
        self.size
    }

    /// Is module (x, y) dark?
    pub fn module(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.size || y >= self.size {
            return false;
        }
        let i = (y * self.size + x) as usize;
        self.bits[i / 8] & (1 << (i % 8)) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_url_and_wifi() {
        let qr = Qr::encode("http://192.168.4.1/").unwrap();
        assert!(qr.size() >= 21);
        // Top-left finder pattern: dark border, light ring, dark center.
        assert!(qr.module(0, 0));
        assert!(!qr.module(1, 1));
        assert!(qr.module(3, 3));
        assert!(!qr.module(-1, 0));
        assert!(Qr::encode("WIFI:T:nopass;S:ESP-Chess-1A2B;;").is_some());
    }
}
