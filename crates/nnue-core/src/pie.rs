//! ESP32-S3 SIMD instructions (PIE – "Processor Instruction Extensions", 128-bit registers q0–q7).
//!
//! Enabled by the `simd-s3` feature and used only on Xtensa. The core that runs these
//! instructions must have the coprocessor enabled ([`enable`]) – the firmware does this at the
//! start of the engine thread. The scheduler saves neither the q registers nor ACCX, so on the
//! engine's core only the engine may use them.
//!
//! PIE arithmetic saturates (`vadds`/`vsubs`) while the scalar code wraps. For networks with an
//! i8 FT (QA ≤ 127, at most 32 pieces) accumulator values stay around ±4,000, so the results are
//! identical. Even so, the code is only used after [`self_test`] (a comparison with the scalar
//! version on random data at startup) has passed.
//!
//! Widening i8 → i16: `vcmp.lt.s8` yields sign bytes (0xFF for negatives) and `vzip.8`
//! interleaves them with the data, producing two registers of 8 × i16 (neurons 0–7 and 8–15 of
//! the block).

use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

/// SIMD passed the self-test and may be used.
static READY: AtomicBool = AtomicBool::new(false);

#[inline(always)]
pub(crate) fn ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Enables the coprocessors (including PIE) for the current thread on the current core.
pub fn enable() {
    // SAFETY: CPENABLE only allows coprocessor instructions; interrupt handlers save and restore it.
    unsafe {
        asm!(
            "rsr.cpenable {t}",
            "movi {m}, 0xff",
            "or {t}, {t}, {m}",
            "wsr.cpenable {t}",
            "rsync",
            t = out(reg) _,
            m = out(reg) _,
        );
    }
}

/// `dst = src − rs + ra` in blocks of 16 neurons (`n` = number of neurons, a multiple of 16).
/// Pointers aligned to 16 bytes; `dst` may equal `src` (in-place update).
#[inline(always)]
pub(crate) unsafe fn sub_add(
    dst: *mut i16,
    src: *const i16,
    rs: *const i8,
    ra: *const i8,
    n: usize,
) {
    let blocks = n / 16;
    // SAFETY: the caller guarantees the pointers are valid and aligned for `n` elements.
    unsafe {
        asm!(
            "ee.zero.q q7",
            "loopnez {blocks}, 2f",
            "ee.vld.128.ip q0, {src}, 16",
            "ee.vld.128.ip q1, {src}, 16",
            "ee.vld.128.ip q2, {rs}, 16",
            "ee.vcmp.lt.s8 q3, q2, q7",
            "ee.vzip.8 q2, q3",
            "ee.vsubs.s16 q0, q0, q2",
            "ee.vsubs.s16 q1, q1, q3",
            "ee.vld.128.ip q4, {ra}, 16",
            "ee.vcmp.lt.s8 q5, q4, q7",
            "ee.vzip.8 q4, q5",
            "ee.vadds.s16 q0, q0, q4",
            "ee.vadds.s16 q1, q1, q5",
            "ee.vst.128.ip q0, {dst}, 16",
            "ee.vst.128.ip q1, {dst}, 16",
            "2:",
            blocks = in(reg) blocks,
            src = inout(reg) src => _,
            rs = inout(reg) rs => _,
            ra = inout(reg) ra => _,
            dst = inout(reg) dst => _,
            options(nostack),
        );
    }
}

/// Σ t·(t·w), t = clamp(v, 0, qa), over `n` neurons (a multiple of 16); `w` = i8 weights.
/// Pointers aligned to 16 bytes. The sum must fit in i32 (`small_out` networks).
#[inline(always)]
pub(crate) unsafe fn dot_small(acc: *const i16, w: *const i8, qa: i16, n: usize) -> i32 {
    let blocks = n / 16;
    let qa_word = [qa; 8];
    let sum: i32;
    // SAFETY: the caller guarantees the pointers are valid and aligned for `n` elements.
    unsafe {
        asm!(
            "ee.zero.q q7",
            "ee.vldbc.16 q6, {qa}",
            "ee.zero.accx",
            "ssai 0",
            "loopnez {blocks}, 2f",
            "ee.vld.128.ip q0, {acc}, 16",
            "ee.vld.128.ip q1, {acc}, 16",
            "ee.vmax.s16 q0, q0, q7",
            "ee.vmin.s16 q0, q0, q6",
            "ee.vmax.s16 q1, q1, q7",
            "ee.vmin.s16 q1, q1, q6",
            "ee.vld.128.ip q2, {w}, 16",
            "ee.vcmp.lt.s8 q3, q2, q7",
            "ee.vzip.8 q2, q3",
            "ee.vmul.s16 q2, q0, q2",
            "ee.vmul.s16 q3, q1, q3",
            "ee.vmulas.s16.accx q0, q2",
            "ee.vmulas.s16.accx q1, q3",
            "2:",
            "rur.accx_0 {sum}",
            blocks = in(reg) blocks,
            qa = in(reg) qa_word.as_ptr(),
            acc = inout(reg) acc => _,
            w = inout(reg) w => _,
            sum = lateout(reg) sum,
            options(nostack),
        );
    }
    sum
}

/// Compares SIMD with the scalar computation on pseudo-random data (including extremes:
/// clamping at 0 and QA, negative weights). If they agree, enables SIMD and returns `true`.
/// Call after [`enable`].
pub fn self_test() -> bool {
    #[repr(C, align(16))]
    struct A<T>(T);
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut ok = true;
    for _ in 0..64 {
        let mut src = A([0i16; 64]);
        let mut rs = A([0i8; 64]);
        let mut ra = A([0i8; 64]);
        for i in 0..64 {
            src.0[i] = (next() % 9001) as i16 - 4500;
            rs.0[i] = next() as i8;
            ra.0[i] = next() as i8;
        }
        // sub_add into a separate buffer and in place
        let mut dst = A([0i16; 64]);
        unsafe {
            sub_add(
                dst.0.as_mut_ptr(),
                src.0.as_ptr(),
                rs.0.as_ptr(),
                ra.0.as_ptr(),
                64,
            )
        };
        let mut inplace = A(src.0);
        let p = inplace.0.as_mut_ptr();
        unsafe { sub_add(p, p, rs.0.as_ptr(), ra.0.as_ptr(), 64) };
        for i in 0..64 {
            let expected = src.0[i]
                .wrapping_sub(rs.0[i] as i16)
                .wrapping_add(ra.0[i] as i16);
            ok &= dst.0[i] == expected && inplace.0[i] == expected;
        }
        // dot_small: values around the clamp (negative, 0..qa, above qa)
        let mut acc = A([0i16; 64]);
        let mut w = A([0i8; 64]);
        for i in 0..64 {
            acc.0[i] = (next() % 400) as i16 - 150;
            w.0[i] = ((next() % 255) as i32 - 127) as i8;
        }
        let qa = 127i16;
        let expected: i32 = (0..64)
            .map(|i| {
                let t = acc.0[i].clamp(0, qa) as i32;
                t * (t * w.0[i] as i32)
            })
            .sum();
        let got = unsafe { dot_small(acc.0.as_ptr(), w.0.as_ptr(), qa, 64) };
        ok &= got == expected;
    }
    READY.store(ok, Ordering::Relaxed);
    ok
}
