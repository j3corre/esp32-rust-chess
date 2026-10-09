//! Flash record format for stored Wi-Fi credentials.
//!
//! The record has a fixed size of 128 B and a CRC-32 checksum – a record
//! corrupted by a power loss mid-write is detected and ignored.
//!
//! ```text
//! 0..4    "CHS1"          (magic number + format version)
//! 4       SSID length     (0–32)
//! 5       password length (0–64)
//! 6..38   SSID
//! 38..102 password
//! 102..124 reserved (zeros)
//! 124..128 CRC-32 of bytes 0..124 (little-endian)
//! ```

use heapless::String;

pub const RECORD_SIZE: usize = 128;
const MAGIC: &[u8; 4] = b"CHS1";

/// Stored credentials for the home Wi-Fi network.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WifiCredentials {
    pub ssid: String<32>,
    pub password: String<64>,
}

impl WifiCredentials {
    /// Encodes into a 128-byte record.
    pub fn encode(&self) -> [u8; RECORD_SIZE] {
        let mut record = [0u8; RECORD_SIZE];
        record[..4].copy_from_slice(MAGIC);
        record[4] = self.ssid.len() as u8;
        record[5] = self.password.len() as u8;
        record[6..6 + self.ssid.len()].copy_from_slice(self.ssid.as_bytes());
        record[38..38 + self.password.len()].copy_from_slice(self.password.as_bytes());
        let crc = crc32(&record[..124]);
        record[124..].copy_from_slice(&crc.to_le_bytes());
        record
    }

    /// Decodes a record; `None` if empty (erased flash = 0xFF) or corrupted.
    pub fn decode(record: &[u8]) -> Option<WifiCredentials> {
        if record.len() < RECORD_SIZE || &record[..4] != MAGIC {
            return None;
        }
        let crc = u32::from_le_bytes([record[124], record[125], record[126], record[127]]);
        if crc != crc32(&record[..124]) {
            return None;
        }
        let ssid_len = record[4] as usize;
        let pass_len = record[5] as usize;
        if ssid_len == 0 || ssid_len > 32 || pass_len > 64 {
            return None;
        }
        let ssid = core::str::from_utf8(&record[6..6 + ssid_len]).ok()?;
        let password = core::str::from_utf8(&record[38..38 + pass_len]).ok()?;
        Some(WifiCredentials {
            ssid: String::try_from(ssid).ok()?,
            password: String::try_from(password).ok()?,
        })
    }
}

/// CRC-32 (IEEE, as in zip/PNG), computed bitwise – the record is small.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(ssid: &str, password: &str) -> WifiCredentials {
        WifiCredentials {
            ssid: String::try_from(ssid).unwrap(),
            password: String::try_from(password).unwrap(),
        }
    }

    #[test]
    fn roundtrip() {
        let original = creds("Home Wi-Fi ü", "secretpass123");
        let record = original.encode();
        assert_eq!(WifiCredentials::decode(&record), Some(original));
        let open = creds("Cafe", "");
        assert_eq!(WifiCredentials::decode(&open.encode()), Some(open));
    }

    #[test]
    fn rejects_erased_and_corrupted_records() {
        assert_eq!(WifiCredentials::decode(&[0xFF; RECORD_SIZE]), None);
        let mut record = creds("Net", "password1").encode();
        record[10] ^= 1;
        assert_eq!(WifiCredentials::decode(&record), None);
    }

    #[test]
    fn crc_matches_reference() {
        // Known CRC-32 check value for "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
