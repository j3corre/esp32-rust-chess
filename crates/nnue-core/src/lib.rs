//! NNUE inference for the ESP32 chess engine – `no_std`, no FPU, weights read directly from flash.
//!
//! Supported formats:
//! - **legacy** (Dog / tinychess / bullet `quantised.bin`): no header, `(768→H)×2→1`, i16,
//!   QA=255, QB=64. The computation is bit-for-bit identical to the original engine (including
//!   the i16 wrap-around trick and i32 overflow).
//! - **v1** (64-byte header `ESPNNUE\0`): any H, output buckets by material,
//!   FT weights i16 or i8, optional hidden layers L1 (i8) and L2 (i16).
//!
//! Inputs are always Chess768: `64*kind + sq` for own pieces, `384 + …` for the opponent's;
//! the black perspective is a vertical flip (`sq ^ 56`).

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(
    all(feature = "simd-s3", target_arch = "xtensa"),
    feature(asm_experimental_arch)
)]

#[cfg(target_endian = "big")]
compile_error!("weights are little-endian");

#[cfg(feature = "std")]
pub mod board;
pub mod format;
/// ESP32-S3 SIMD (PIE).
#[cfg(all(feature = "simd-s3", target_arch = "xtensa"))]
pub mod pie;

pub use format::{Arch, FormatError, FtType, Header};

/// Highest non-mate value; the output is clamped to it.
pub const MAX_NON_MATE: i32 = 29_500;
/// Max. width of the hidden layers and max. number of buckets (stack buffers).
pub const MAX_LAYER: usize = 32;
pub const MAX_BUCKETS: usize = 8;

pub const WHITE: usize = 0;
pub const BLACK: usize = 1;

/// Input index of a piece (color 0/1, kind 0..6 = P N B R Q K) on square `sq` (a1 = 0) as seen
/// from `perspective`.
#[inline(always)]
pub fn feature_index(perspective: usize, color: usize, kind: usize, sq: usize) -> usize {
    let sq = if perspective == WHITE { sq } else { sq ^ 56 };
    let side = if color == perspective { 0 } else { 384 };
    side + kind * 64 + sq
}

/// Output bucket by piece count (same as bullet's `MaterialCount<N>`).
#[inline(always)]
pub fn output_bucket(piece_count: u32, buckets: usize) -> usize {
    if buckets <= 1 {
        return 0;
    }
    let divisor = 32usize.div_ceil(buckets);
    (piece_count as usize - 2) / divisor
}

#[inline(always)]
fn read_i16(b: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([b[off], b[off + 1]])
}

#[inline(always)]
fn read_i32(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// `v · scale / div` (division rounding toward zero). Done in i32 when it fits – on RV32 i32
/// division is a single instruction, i64 division a library call. Same result as pure i64.
#[inline(always)]
fn scale_out(v: i64, scale: i32, div: i32) -> i32 {
    if let Some(p) = i32::try_from(v).ok().and_then(|v| v.checked_mul(scale)) {
        return p / div;
    }
    (v * scale as i64 / div as i64) as i32
}

/// Unrolling of the i8 FT update loops: 8 neurons per pass (LLVM does not unroll these loops for
/// RISC-V, and the loop overhead – pointer increments and the branch – would otherwise be half
/// of the instructions).
const UNROLL: usize = 8;

/// `dst = src − rs + ra` (i8 FT rows, i16 accumulator, wrapping on overflow).
#[inline(always)]
fn i8_sub_add_from<const H: usize>(dst: &mut [i16; H], src: &[i16; H], rs: &[i8; H], ra: &[i8; H]) {
    let d = dst.as_chunks_mut::<UNROLL>().0;
    let s = src.as_chunks::<UNROLL>().0;
    let rs = rs.as_chunks::<UNROLL>().0;
    let ra = ra.as_chunks::<UNROLL>().0;
    for (((d, s), rs), ra) in d.iter_mut().zip(s).zip(rs).zip(ra) {
        for j in 0..UNROLL {
            d[j] = s[j].wrapping_sub(rs[j] as i16).wrapping_add(ra[j] as i16);
        }
    }
}

/// `acc = acc − rs + ra` in place.
#[inline(always)]
fn i8_sub_add<const H: usize>(acc: &mut [i16; H], rs: &[i8; H], ra: &[i8; H]) {
    let a = acc.as_chunks_mut::<UNROLL>().0;
    let rs = rs.as_chunks::<UNROLL>().0;
    let ra = ra.as_chunks::<UNROLL>().0;
    for ((a, rs), ra) in a.iter_mut().zip(rs).zip(ra) {
        for j in 0..UNROLL {
            a[j] = a[j].wrapping_sub(rs[j] as i16).wrapping_add(ra[j] as i16);
        }
    }
}

/// `acc ± row` in place.
#[inline(always)]
fn i8_add_row<const H: usize, const ADD: bool>(acc: &mut [i16; H], row: &[i8; H]) {
    let a = acc.as_chunks_mut::<UNROLL>().0;
    let r = row.as_chunks::<UNROLL>().0;
    for (a, r) in a.iter_mut().zip(r) {
        for j in 0..UNROLL {
            a[j] = if ADD {
                a[j].wrapping_add(r[j] as i16)
            } else {
                a[j].wrapping_sub(r[j] as i16)
            };
        }
    }
}

/// Σ t·(t·w), t = clamp(v, 0, qa), for small weights (|w| ≤ 127, qa ≤ 127) – exact in i32.
/// Processes groups of four with two partial sums (less loop overhead, independent multiplies).
#[inline(always)]
fn dot_small<const H: usize>(acc: &[i16; H], w: &[i8], qa: i16) -> i32 {
    let (mut s0, mut s1) = (0i32, 0i32);
    #[inline(always)]
    fn clamp(x: i16, qa: i16) -> i32 {
        x.clamp(0, qa) as i32
    }
    #[inline(always)]
    fn term(t: i32, w: i8) -> i32 {
        t * (t * w as i32)
    }
    for (v, w) in acc.as_chunks::<4>().0.iter().zip(w.as_chunks::<4>().0) {
        let (t0, t1, t2, t3) = (
            clamp(v[0], qa),
            clamp(v[1], qa),
            clamp(v[2], qa),
            clamp(v[3], qa),
        );
        s0 += term(t0, w[0]) + term(t2, w[2]);
        s1 += term(t1, w[1]) + term(t3, w[3]);
    }
    s0 + s1
}

/// A network over bytes in flash (nothing is copied to RAM). `H` = FT hidden layer size.
#[derive(Clone, Copy)]
pub struct Network<'a, const H: usize> {
    bytes: &'a [u8],
    pub arch: Arch,
}

impl<'a, const H: usize> Network<'a, H> {
    /// Loads a network (legacy by size, otherwise v1 header). `bytes` must be 4-byte aligned.
    pub fn new(bytes: &'a [u8]) -> Result<Self, FormatError> {
        let arch = Arch::detect(bytes)?;
        if arch.hidden != H {
            return Err(FormatError::HiddenMismatch {
                file: arch.hidden,
                expected: H,
            });
        }
        if !(bytes.as_ptr() as usize).is_multiple_of(4) {
            return Err(FormatError::Misaligned);
        }
        Ok(Self { bytes, arch })
    }

    /// A network with a known architecture (typically from a build script) – no checks, usable
    /// in a `static`. The caller guarantees that `bytes` match `arch`, are 4-byte aligned and
    /// that `arch.hidden == H`.
    pub const fn from_arch(bytes: &'a [u8], arch: Arch) -> Self {
        assert!(arch.hidden == H);
        assert!(bytes.len() >= arch.total_len);
        Self { bytes, arch }
    }

    /// FT weight row as u32 words (i16: 2 values per word, i8: 4 values per word).
    #[inline(always)]
    fn ft_row_words(&self, feature: usize) -> &'a [u32] {
        let row_bytes = H * self.arch.ft_type.bytes();
        let off = self.arch.ft_offset + feature * row_bytes;
        let b = &self.bytes[off..off + row_bytes];
        // SAFETY: offset and length are multiples of 4 (H is a multiple of 8, sections aligned
        // to 64 bytes), the network start is aligned (checked in `new`), u32 has no invalid bit
        // patterns.
        unsafe { core::slice::from_raw_parts(b.as_ptr() as *const u32, row_bytes / 4) }
    }

    /// FT weight row as i8 (FtType::I8 only).
    #[inline(always)]
    fn ft_row_i8(&self, feature: usize) -> &'a [i8; H] {
        let off = self.arch.ft_offset + feature * H;
        let b = &self.bytes[off..off + H];
        // SAFETY: i8 has no alignment requirements; length = H values.
        unsafe { &*(b.as_ptr() as *const [i8; H]) }
    }

    /// `n` i16 values starting at byte offset `off` (output weights).
    #[inline(always)]
    fn i16_at(&self, off: usize, n: usize) -> &'a [i16] {
        let b = &self.bytes[off..off + n * 2];
        // SAFETY: as above – even offset in an aligned network.
        unsafe { core::slice::from_raw_parts(b.as_ptr() as *const i16, n) }
    }

    /// Number of output weights for [`Accumulator::evaluate_small`] (all buckets), or 0 when
    /// the network has no small output weights ([`Arch::small_out`]).
    pub const fn small_out_len(arch: &Arch) -> usize {
        if arch.small_out {
            arch.out_buckets * 2 * H
        } else {
            0
        }
    }

    /// Copies the output weights (|w| ≤ 127) as i8 into `dst` – a RAM copy for
    /// [`Accumulator::evaluate_small`] (read from RAM instead of the flash cache, half the size).
    pub fn small_out_weights(&self, dst: &mut [i8]) {
        assert!(self.arch.small_out && dst.len() == Self::small_out_len(&self.arch));
        let src = self.i16_at(self.arch.out_w_offset, dst.len());
        for (d, &w) in dst.iter_mut().zip(src) {
            *d = w as i8;
        }
    }

    #[inline(always)]
    fn ft_bias(&self, i: usize) -> i16 {
        read_i16(self.bytes, self.arch.ft_bias_offset + 2 * i)
    }
}

// ---------------------------------------------------------------------------
// SWAR arithmetic: 2 × i16 in one u32 (the ESP32-C6 has no SIMD)
// ---------------------------------------------------------------------------

const LOW15: u32 = 0x7FFF_7FFF;
const HIGH: u32 = 0x8000_8000;

#[inline(always)]
fn add_lanes(a: u32, b: u32) -> u32 {
    ((a & LOW15) + (b & LOW15)) ^ ((a ^ b) & HIGH)
}

#[inline(always)]
fn sub_lanes(a: u32, b: u32) -> u32 {
    ((a | HIGH) - (b & LOW15)) ^ ((a ^ !b) & HIGH)
}

/// Accumulators for both perspectives (i16, processed in pairs as u32 – SWAR).
#[derive(Clone)]
#[repr(C, align(16))]
pub struct Accumulator<const H: usize> {
    values: [[i16; H]; 2],
}

/// A single board change: (color, kind, square).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PieceSq {
    pub color: u8,
    pub kind: u8,
    pub sq: u8,
}

impl<const H: usize> Default for Accumulator<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const H: usize> Accumulator<H> {
    const _H_OK: () = assert!(H.is_multiple_of(16) && H > 0, "H must be a multiple of 16");

    pub const fn new() -> Self {
        #[allow(clippy::let_unit_value)]
        let _ = Self::_H_OK;
        Self {
            values: [[0; H]; 2],
        }
    }

    /// View of one accumulator as H/2 u32 words.
    #[inline(always)]
    fn words(&mut self, persp: usize) -> &mut [u32] {
        // SAFETY: the struct is 16-byte aligned, a row is H·2 bytes (a multiple of 4), i16 and
        // u32 have no invalid bit patterns. On LE the low half of a word is the even index.
        unsafe {
            core::slice::from_raw_parts_mut(self.values[persp].as_mut_ptr() as *mut u32, H / 2)
        }
    }

    /// Computes both perspectives from scratch.
    pub fn refresh(&mut self, net: &Network<H>, pieces: impl Iterator<Item = PieceSq> + Clone) {
        for p in 0..2 {
            for i in 0..H {
                self.values[p][i] = net.ft_bias(i);
            }
        }
        for pc in pieces {
            self.add(net, pc);
        }
    }

    #[inline(always)]
    fn row_op(&mut self, net: &Network<H>, persp: usize, feature: usize, add: bool) {
        match net.arch.ft_type {
            FtType::I16 => {
                let row = net.ft_row_words(feature);
                let acc = self.words(persp);
                for (a, &w) in acc.iter_mut().zip(row.iter()) {
                    *a = if add {
                        add_lanes(*a, w)
                    } else {
                        sub_lanes(*a, w)
                    };
                }
            }
            FtType::I8 => {
                let row8 = net.ft_row_i8(feature);
                let acc16 = &mut self.values[persp];
                if add {
                    i8_add_row::<H, true>(acc16, row8);
                } else {
                    i8_add_row::<H, false>(acc16, row8);
                }
            }
        }
    }

    /// Adds a piece (both perspectives).
    #[cfg_attr(not(all(feature = "iram", target_os = "none")), inline)]
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        link_section = ".rwtext",
        inline(never)
    )]
    pub fn add(&mut self, net: &Network<H>, p: PieceSq) {
        for persp in 0..2 {
            let f = feature_index(persp, p.color as usize, p.kind as usize, p.sq as usize);
            self.row_op(net, persp, f, true);
        }
    }

    /// Removes a piece (both perspectives).
    #[cfg_attr(not(all(feature = "iram", target_os = "none")), inline)]
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        link_section = ".rwtext",
        inline(never)
    )]
    pub fn remove(&mut self, net: &Network<H>, p: PieceSq) {
        for persp in 0..2 {
            let f = feature_index(persp, p.color as usize, p.kind as usize, p.sq as usize);
            self.row_op(net, persp, f, false);
        }
    }

    /// A plain move (1 removal + 1 addition) in a single pass.
    #[cfg_attr(not(all(feature = "iram", target_os = "none")), inline)]
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        link_section = ".rwtext",
        inline(never)
    )]
    pub fn sub_add(&mut self, net: &Network<H>, removed: PieceSq, added: PieceSq) {
        if net.arch.ft_type == FtType::I8 {
            // single pass: fewer accumulator reads/writes and half the weight traffic from flash
            for persp in 0..2 {
                let f_sub = feature_index(
                    persp,
                    removed.color as usize,
                    removed.kind as usize,
                    removed.sq as usize,
                );
                let f_add = feature_index(
                    persp,
                    added.color as usize,
                    added.kind as usize,
                    added.sq as usize,
                );
                let (rs, ra) = (net.ft_row_i8(f_sub), net.ft_row_i8(f_add));
                #[cfg(all(feature = "simd-s3", target_arch = "xtensa"))]
                if pie::ready() {
                    let p = self.values[persp].as_mut_ptr();
                    // SAFETY: see `sub_add_from`; in place is allowed (a block is loaded before
                    // it is stored).
                    unsafe { pie::sub_add(p, p, rs.as_ptr(), ra.as_ptr(), H) };
                    continue;
                }
                i8_sub_add(&mut self.values[persp], rs, ra);
            }
            return;
        }
        for persp in 0..2 {
            let f_sub = feature_index(
                persp,
                removed.color as usize,
                removed.kind as usize,
                removed.sq as usize,
            );
            let f_add = feature_index(
                persp,
                added.color as usize,
                added.kind as usize,
                added.sq as usize,
            );
            let (sub, add) = (net.ft_row_words(f_sub), net.ft_row_words(f_add));
            let acc = self.words(persp);
            for i in 0..H / 2 {
                acc[i] = add_lanes(sub_lanes(acc[i], sub[i]), add[i]);
            }
        }
    }

    /// A plain move computed from the parent: `self = parent − row(removed) + row(added)` in a
    /// single pass (accumulator stack – going back is then free, no `undo` needed).
    #[cfg_attr(not(all(feature = "iram", target_os = "none")), inline)]
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        link_section = ".rwtext",
        inline(never)
    )]
    pub fn sub_add_from(
        &mut self,
        parent: &Self,
        net: &Network<H>,
        removed: PieceSq,
        added: PieceSq,
    ) {
        for persp in 0..2 {
            let f_sub = feature_index(
                persp,
                removed.color as usize,
                removed.kind as usize,
                removed.sq as usize,
            );
            let f_add = feature_index(
                persp,
                added.color as usize,
                added.kind as usize,
                added.sq as usize,
            );
            let src = &parent.values[persp];
            let dst = &mut self.values[persp];
            match net.arch.ft_type {
                FtType::I8 => {
                    let (rs, ra) = (net.ft_row_i8(f_sub), net.ft_row_i8(f_add));
                    #[cfg(all(feature = "simd-s3", target_arch = "xtensa"))]
                    if pie::ready() {
                        // SAFETY: rows and accumulators have H elements (a multiple of 16) and
                        // are 16-byte aligned (accumulator is align(16), FT rows start at
                        // multiples of H from a 64-byte boundary).
                        unsafe {
                            pie::sub_add(
                                dst.as_mut_ptr(),
                                src.as_ptr(),
                                rs.as_ptr(),
                                ra.as_ptr(),
                                H,
                            )
                        };
                        continue;
                    }
                    i8_sub_add_from(dst, src, rs, ra);
                }
                FtType::I16 => {
                    let (rs, ra) = (net.ft_row_words(f_sub), net.ft_row_words(f_add));
                    // SAFETY: same alignment and length as in `words()` (H/2 words of 2 × i16).
                    let (sw, dw) = unsafe {
                        (
                            core::slice::from_raw_parts(src.as_ptr() as *const u32, H / 2),
                            core::slice::from_raw_parts_mut(dst.as_mut_ptr() as *mut u32, H / 2),
                        )
                    };
                    for i in 0..H / 2 {
                        dw[i] = add_lanes(sub_lanes(sw[i], rs[i]), ra[i]);
                    }
                }
            }
        }
    }

    /// Copies the parent's accumulator (for moves that are not 1 removal + 1 addition).
    #[inline]
    pub fn copy_from(&mut self, parent: &Self) {
        self.values = parent.values;
    }

    #[inline(always)]
    fn lane(&self, persp: usize, i: usize) -> i16 {
        self.values[persp][i]
    }

    /// Accumulator values of one perspective (tests / tools).
    pub fn lanes(&self, persp: usize) -> impl Iterator<Item = i16> + '_ {
        (0..H).map(move |i| self.lane(persp, i))
    }

    /// Evaluation from the side to move's point of view in centipawns. `piece_count` = number
    /// of pieces on the board (including kings).
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        link_section = ".rwtext",
        inline(never)
    )]
    pub fn evaluate(&self, net: &Network<H>, side_to_move: usize, piece_count: u32) -> i32 {
        let a = &net.arch;
        let us = side_to_move;
        let them = 1 - side_to_move;
        if a.legacy {
            return self.evaluate_legacy(net, us, them);
        }
        let bucket = output_bucket(piece_count, a.out_buckets);
        let qa = a.qa;
        let out = if a.l1 == 0 {
            // SCReLU → output, i16 weights with QB: Σ t·(t·w), exact in i64.
            let weights = net.i16_at(a.out_w_offset + bucket * 2 * H * 2, 2 * H);
            let mut sum: i64 = 0;
            if a.small_out {
                // |w| ≤ 127, t ≤ 127, 2H ≤ 1024 terms → the whole sum fits exactly in i32.
                let mut part: i32 = 0;
                for (p, base) in [(us, 0), (them, H)] {
                    for (&v, &w) in self.values[p].iter().zip(weights[base..base + H].iter()) {
                        let t = v.clamp(0, qa as i16) as i32;
                        part += t * (t * w as i32);
                    }
                }
                sum = part as i64;
            } else if qa <= 64 {
                // A term is ≤ 64²·32767 ≈ 1.34e8 → a block of 8 terms fits in i32; i64 only once
                // per block (i64 addition is expensive on RV32). Exactly the same result as i64.
                sum = self.dot_blocks::<8>(weights, us, them, qa);
            } else if qa <= 128 {
                // A term is ≤ 128²·32767 ≈ 5.4e8 → a block of 4 terms (≤ 2.15e9) fits in i32.
                sum = self.dot_blocks::<4>(weights, us, them, qa);
            } else {
                for (p, base) in [(us, 0), (them, H)] {
                    for (&v, &w) in self.values[p].iter().zip(weights[base..base + H].iter()) {
                        let t = v.clamp(0, qa as i16) as i32;
                        sum += (t * (t * w as i32)) as i64;
                    }
                }
            }
            let bias = read_i32(net.bytes, a.out_b_offset + 4 * bucket) as i64;
            let v = sum / qa as i64 + bias;
            scale_out(v, a.scale, qa * a.qb)
        } else {
            self.evaluate_deep(net, us, them, bucket)
        };
        out.clamp(-MAX_NON_MATE, MAX_NON_MATE)
    }

    /// Same as [`Accumulator::evaluate`] for networks with [`Arch::small_out`], but with the
    /// output weights in RAM (`weights` = [`Network::small_out_weights`]). Bit-for-bit identical.
    #[cfg_attr(
        all(feature = "iram", target_os = "none"),
        link_section = ".rwtext",
        inline(never)
    )]
    pub fn evaluate_small(
        &self,
        net: &Network<H>,
        side_to_move: usize,
        piece_count: u32,
        weights: &[i8],
    ) -> i32 {
        let a = &net.arch;
        debug_assert!(a.small_out && weights.len() == Network::<H>::small_out_len(a));
        let bucket = output_bucket(piece_count, a.out_buckets);
        let w = &weights[bucket * 2 * H..(bucket + 1) * 2 * H];
        let qa = a.qa as i16;
        #[cfg(all(feature = "simd-s3", target_arch = "xtensa"))]
        let sum = if pie::ready() && (w.as_ptr() as usize).is_multiple_of(16) {
            // SAFETY: H neurons (a multiple of 16), accumulator and weights (checked) are
            // 16-byte aligned.
            unsafe {
                pie::dot_small(self.values[side_to_move].as_ptr(), w.as_ptr(), qa, H)
                    + pie::dot_small(
                        self.values[1 - side_to_move].as_ptr(),
                        w[H..].as_ptr(),
                        qa,
                        H,
                    )
            }
        } else {
            dot_small::<H>(&self.values[side_to_move], &w[..H], qa)
                + dot_small::<H>(&self.values[1 - side_to_move], &w[H..], qa)
        };
        #[cfg(not(all(feature = "simd-s3", target_arch = "xtensa")))]
        let sum = dot_small::<H>(&self.values[side_to_move], &w[..H], qa)
            + dot_small::<H>(&self.values[1 - side_to_move], &w[H..], qa);
        let bias = read_i32(net.bytes, a.out_b_offset + 4 * bucket);
        let v = sum / a.qa + bias;
        scale_out(v as i64, a.scale, a.qa * a.qb).clamp(-MAX_NON_MATE, MAX_NON_MATE)
    }

    /// Σ t·(t·w) in blocks of `B` terms in i32 (the caller guarantees a block fits in i32).
    #[inline(always)]
    fn dot_blocks<const B: usize>(&self, weights: &[i16], us: usize, them: usize, qa: i32) -> i64 {
        let mut sum: i64 = 0;
        for (p, base) in [(us, 0), (them, H)] {
            let acc = &self.values[p];
            let ws = &weights[base..base + H];
            for (va, wa) in acc.as_chunks::<B>().0.iter().zip(ws.as_chunks::<B>().0) {
                let mut part: i32 = 0;
                for (&v, &w) in va.iter().zip(wa.iter()) {
                    let t = v.clamp(0, qa as i16) as i32;
                    part += t * (t * w as i32);
                }
                sum += part as i64;
            }
        }
        sum
    }

    /// Bit-exact copy of the original computation (Dog / tinychess).
    #[inline(always)]
    fn evaluate_legacy(&self, net: &Network<H>, us: usize, them: usize) -> i32 {
        const QA: i32 = 255;
        const QB: i32 = 64;
        const SCALE: i32 = 400;
        let a = &net.arch;
        let weights = net.i16_at(a.out_w_offset, 2 * H);
        let dot = |p: usize, base: usize| -> i32 {
            let mut sum: i32 = 0;
            for (&v, &w) in self.values[p].iter().zip(weights[base..base + H].iter()) {
                let t = v.clamp(0, QA as i16);
                let product = t.wrapping_mul(w);
                sum = sum.wrapping_add(t as i32 * product as i32);
            }
            sum
        };
        let mut output = dot(us, 0).wrapping_add(dot(them, H));
        let bias = read_i16(net.bytes, a.out_b_offset) as i32;
        output /= QA;
        output += bias;
        output *= SCALE;
        output /= QA * QB;
        output.clamp(-MAX_NON_MATE, MAX_NON_MATE)
    }

    /// FT → SCReLU (→ QA units) → L1 (i8, QB) → SCReLU → [L2 (i16, QH) → SCReLU] → output (i16, QH).
    fn evaluate_deep(&self, net: &Network<H>, us: usize, them: usize, bucket: usize) -> i32 {
        let a = &net.arch;
        let (qa, qb, qh) = (a.qa, a.qb, a.qh);
        let b = net.bytes;
        let two_h = 2 * H;

        // L1: z_k = Σ_j x_j · w_kj, x_j = t²/QA ∈ [0, QA]
        let mut z1 = [0i32; MAX_LAYER];
        let w1 = a.l1_w_offset + bucket * a.l1 * two_h;
        for (k, z) in z1.iter_mut().enumerate().take(a.l1) {
            *z = read_i32(b, a.l1_b_offset + 4 * (bucket * a.l1 + k));
        }
        for (p, base) in [(us, 0), (them, H)] {
            for i in 0..H {
                let t = self.lane(p, i).clamp(0, qa as i16) as i32;
                let x = t * t / qa;
                if x == 0 {
                    continue;
                }
                for (k, z) in z1.iter_mut().enumerate().take(a.l1) {
                    *z += x * b[w1 + k * two_h + base + i] as i8 as i32;
                }
            }
        }
        // SCReLU in the integer domain: y = clamp(z, 0, QA·QB)² / (QA·QB²) ∈ [0, QA]
        let mut x1 = [0i32; MAX_LAYER];
        let one = qa * qb;
        for (x, &z) in x1.iter_mut().zip(z1.iter()).take(a.l1) {
            let c = z.clamp(0, one) as i64;
            *x = (c * c / (qa as i64 * qb as i64 * qb as i64)) as i32;
        }

        let (xin, width) = if a.l2 > 0 {
            let mut z2 = [0i32; MAX_LAYER];
            let w2 = a.l2_w_offset + bucket * a.l2 * a.l1 * 2;
            for (k, z) in z2.iter_mut().enumerate().take(a.l2) {
                let mut s = read_i32(b, a.l2_b_offset + 4 * (bucket * a.l2 + k));
                for (j, &x) in x1.iter().enumerate().take(a.l1) {
                    s += x * read_i16(b, w2 + 2 * (k * a.l1 + j)) as i32;
                }
                *z = s;
            }
            let one = qa * qh;
            let mut x2 = [0i32; MAX_LAYER];
            for (x, &z) in x2.iter_mut().zip(z2.iter()).take(a.l2) {
                let c = z.clamp(0, one) as i64;
                *x = (c * c / (qa as i64 * qh as i64 * qh as i64)) as i32;
            }
            (x2, a.l2)
        } else {
            (x1, a.l1)
        };

        let wo = a.out_w_offset + bucket * width * 2;
        let mut s = read_i32(b, a.out_b_offset + 4 * bucket) as i64;
        for (j, &x) in xin.iter().enumerate().take(width) {
            s += x as i64 * read_i16(b, wo + 2 * j) as i64;
        }
        (s * a.scale as i64 / (qa as i64 * qh as i64)) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_blocks_matches_i64() {
        let mut seed = 0xDEAD_BEEFu32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for &qa in &[64i32, 100, 127, 128] {
            for round in 0..200 {
                let mut acc = Accumulator::<128>::new();
                let mut w = [0i16; 256];
                for p in 0..2 {
                    for v in acc.values[p].iter_mut() {
                        // mix of extreme values (saturating at qa) and random ones
                        *v = if round % 3 == 0 {
                            qa as i16 + 5
                        } else {
                            (next() % 400) as i16 - 100
                        };
                    }
                }
                for x in w.iter_mut() {
                    *x = if round % 2 == 0 {
                        i16::MAX
                    } else if round % 5 == 0 {
                        i16::MIN + 1
                    } else {
                        next() as i16
                    };
                }
                let reference: i64 = [(0usize, 0usize), (1, 128)]
                    .iter()
                    .map(|&(p, base)| {
                        (0..128)
                            .map(|i| {
                                let t = acc.values[p][i].clamp(0, qa as i16) as i64;
                                t * t * w[base + i] as i64
                            })
                            .sum::<i64>()
                    })
                    .sum();
                let got = if qa <= 64 {
                    acc.dot_blocks::<8>(&w, 0, 1, qa)
                } else {
                    acc.dot_blocks::<4>(&w, 0, 1, qa)
                };
                assert_eq!(got, reference, "qa={qa} round={round}");
            }
        }
    }

    #[test]
    fn swar_matches_scalar() {
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        let lanes = |x: u32| [x as u16 as i16, (x >> 16) as u16 as i16];
        for _ in 0..50_000 {
            let (a, b) = (next(), next());
            let (la, lb) = (lanes(a), lanes(b));
            let (s, d) = (lanes(add_lanes(a, b)), lanes(sub_lanes(a, b)));
            for k in 0..2 {
                assert_eq!(s[k], la[k].wrapping_add(lb[k]));
                assert_eq!(d[k], la[k].wrapping_sub(lb[k]));
            }
        }
    }
}
