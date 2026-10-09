//! Binary network formats and section offset computation.

/// Weight type of the feature transformer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FtType {
    I16 = 0,
    I8 = 1,
}

impl FtType {
    pub const fn bytes(self) -> usize {
        match self {
            FtType::I16 => 2,
            FtType::I8 => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    TooShort,
    BadMagic,
    BadVersion(u32),
    BadHeader,
    SizeMismatch { file: usize, expected: usize },
    HiddenMismatch { file: usize, expected: usize },
    Misaligned,
}

pub const MAGIC: [u8; 8] = *b"ESPNNUE\0";
pub const VERSION: u32 = 1;
pub const HEADER_LEN: usize = 64;

/// Parameters stored in the v1 header.
///
/// Layout: 8-byte magic, then eleven little-endian `u32` fields: version, hidden, out_buckets,
/// l1, l2, ft_type, qa, qb, qh, scale, total length; zero padding up to 64 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub hidden: usize,
    pub out_buckets: usize,
    pub l1: usize,
    pub l2: usize,
    pub ft_type: FtType,
    /// FT quantization (weights, bias, activation range).
    pub qa: i32,
    /// Quantization of the output layer (without L1) or of the L1 layer.
    pub qb: i32,
    /// Quantization of the deeper layers (L2, output after the hidden layers).
    pub qh: i32,
    pub scale: i32,
}

impl Header {
    pub fn decode(b: &[u8]) -> Result<(Header, usize), FormatError> {
        if b.len() < HEADER_LEN {
            return Err(FormatError::TooShort);
        }
        if b[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let f = |i: usize| {
            u32::from_le_bytes([b[8 + 4 * i], b[9 + 4 * i], b[10 + 4 * i], b[11 + 4 * i]])
        };
        if f(0) != VERSION {
            return Err(FormatError::BadVersion(f(0)));
        }
        let ft_type = match f(5) {
            0 => FtType::I16,
            1 => FtType::I8,
            _ => return Err(FormatError::BadHeader),
        };
        let h = Header {
            hidden: f(1) as usize,
            out_buckets: f(2) as usize,
            l1: f(3) as usize,
            l2: f(4) as usize,
            ft_type,
            qa: f(6) as i32,
            qb: f(7) as i32,
            qh: f(8) as i32,
            scale: f(9) as i32,
        };
        let ok = h.hidden.is_multiple_of(16)
            && h.hidden > 0
            && (1..=crate::MAX_BUCKETS).contains(&h.out_buckets)
            && h.l1 <= crate::MAX_LAYER
            && h.l2 <= crate::MAX_LAYER
            && !(h.l1 == 0 && h.l2 > 0)
            && h.qa > 0
            && h.qb > 0
            && h.qh > 0;
        if !ok {
            return Err(FormatError::BadHeader);
        }
        Ok((h, f(10) as usize))
    }
}

/// Complete network description including section offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arch {
    pub legacy: bool,
    pub hidden: usize,
    pub out_buckets: usize,
    pub l1: usize,
    pub l2: usize,
    pub ft_type: FtType,
    pub qa: i32,
    pub qb: i32,
    pub qh: i32,
    pub scale: i32,
    pub ft_offset: usize,
    pub ft_bias_offset: usize,
    pub l1_w_offset: usize,
    pub l1_b_offset: usize,
    pub l2_w_offset: usize,
    pub l2_b_offset: usize,
    pub out_w_offset: usize,
    pub out_b_offset: usize,
    /// Data length including header and padding (multiple of 64).
    pub total_len: usize,
    /// The output layer (no hidden layers) has all |w| ≤ 127 and QA ≤ 127 (H ≤ 512):
    /// Σ t·(t·w) then fits exactly in i32 and evaluate does not need i64 sums.
    /// Determined from the weights in [`Arch::detect`] (at build time for the board).
    pub small_out: bool,
}

const fn align64(x: usize) -> usize {
    x.div_ceil(64) * 64
}

/// Length of a legacy file (Dog / bullet) for a given H, including padding to 64 bytes.
pub const fn legacy_len(h: usize) -> usize {
    align64(768 * h * 2 + h * 2 + 2 * h * 2 + 2)
}

impl Arch {
    /// Legacy format: `l0w[768][H] i16, l0b[H], l1w[2][H], l1b i16`, no header.
    pub const fn legacy(h: usize) -> Arch {
        let ft = 0;
        let ftb = 768 * h * 2;
        let ow = ftb + h * 2;
        let ob = ow + 2 * h * 2;
        Arch {
            legacy: true,
            hidden: h,
            out_buckets: 1,
            l1: 0,
            l2: 0,
            ft_type: FtType::I16,
            qa: 255,
            qb: 64,
            qh: 1,
            scale: 400,
            ft_offset: ft,
            ft_bias_offset: ftb,
            l1_w_offset: 0,
            l1_b_offset: 0,
            l2_w_offset: 0,
            l2_b_offset: 0,
            out_w_offset: ow,
            out_b_offset: ob,
            total_len: legacy_len(h),
            small_out: false,
        }
    }

    /// v1 layout for the given header.
    pub const fn from_header(h: &Header) -> Arch {
        let n = h.out_buckets;
        let hid = h.hidden;
        let mut off = HEADER_LEN;
        let ft = off;
        off = align64(off + 768 * hid * h.ft_type.bytes());
        let ftb = off;
        off = align64(off + hid * 2);
        let (mut l1w, mut l1b, mut l2w, mut l2b) = (0, 0, 0, 0);
        let width;
        if h.l1 == 0 {
            width = 2 * hid;
        } else {
            l1w = off;
            off = align64(off + n * h.l1 * 2 * hid);
            l1b = off;
            off = align64(off + n * h.l1 * 4);
            if h.l2 > 0 {
                l2w = off;
                off = align64(off + n * h.l2 * h.l1 * 2);
                l2b = off;
                off = align64(off + n * h.l2 * 4);
                width = h.l2;
            } else {
                width = h.l1;
            }
        }
        let ow = off;
        off = align64(off + n * width * 2);
        let ob = off;
        off = align64(off + n * 4);
        Arch {
            legacy: false,
            hidden: hid,
            out_buckets: n,
            l1: h.l1,
            l2: h.l2,
            ft_type: h.ft_type,
            qa: h.qa,
            qb: h.qb,
            qh: h.qh,
            scale: h.scale,
            ft_offset: ft,
            ft_bias_offset: ftb,
            l1_w_offset: l1w,
            l1_b_offset: l1b,
            l2_w_offset: l2w,
            l2_b_offset: l2b,
            out_w_offset: ow,
            out_b_offset: ob,
            total_len: off,
            small_out: false,
        }
    }

    /// Detects the format: v1 header, otherwise legacy by file size.
    pub fn detect(b: &[u8]) -> Result<Arch, FormatError> {
        if b.len() >= HEADER_LEN && b[..8] == MAGIC {
            let (h, total) = Header::decode(b)?;
            let mut a = Arch::from_header(&h);
            if total != a.total_len || b.len() < a.total_len {
                return Err(FormatError::SizeMismatch {
                    file: b.len(),
                    expected: a.total_len,
                });
            }
            a.small_out = small_output_weights(b, &a);
            return Ok(a);
        }
        // legacy: 1542·H + 2 bytes of data, padded to 64
        let guess = b.len() / 1542;
        for h in [guess.saturating_sub(1), guess] {
            if h > 0 && h % 16 == 0 && legacy_len(h) == b.len() {
                return Ok(Arch::legacy(h));
            }
        }
        Err(FormatError::BadMagic)
    }
}

/// Are all output weights small (|w| ≤ 127) and QA ≤ 127? Then 2H · QA² · 127 < 2^31 (H ≤ 512).
fn small_output_weights(b: &[u8], a: &Arch) -> bool {
    if a.legacy || a.l1 != 0 || a.qa > 127 || a.hidden > 512 {
        return false;
    }
    let n = a.out_buckets * 2 * a.hidden;
    (0..n).all(|i| {
        let off = a.out_w_offset + 2 * i;
        let w = i16::from_le_bytes([b[off], b[off + 1]]);
        (-127..=127).contains(&w)
    })
}
