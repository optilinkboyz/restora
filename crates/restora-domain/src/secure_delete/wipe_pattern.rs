//! Wipe patterns: what bytes to overwrite freed space with, and how many
//! passes.
//!
//! This module only generates bytes — it never touches disk. Actually
//! writing those bytes somewhere is `restora-application`'s
//! `wipe_job.rs`'s job, using `restora-infra`'s `WritableByteSource`. That
//! split mirrors everything else in this codebase: domain crate = pure
//! logic, infra crate = I/O, application crate = orchestration.
//!
//! **On pattern choice**, worth having context for: the old "35-pass
//! Gutmann method" and even the common "DoD 5220.22-M 3-pass" were
//! designed for the specific magnetic encoding schemes (MFM/RLL) of
//! drives from decades ago — modern NIST SP 800-88 guidance is that a
//! single well-distributed overwrite pass is sufficient for HDDs, and
//! that overwrite-based wiping is *unreliable on SSDs at all* (wear
//! leveling means the same logical address often maps to a different
//! physical cell after each write) — TRIM/Deallocate or crypto-erase are
//! the actually-correct tools there. We still offer the 3-pass DoD
//! pattern here because people reasonably expect to see it, but the CLI
//! surfaces this context rather than presenting it as strictly better.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassFill {
    Zero,
    One,
    Random,
}

#[derive(Debug, Clone, Copy)]
pub struct WipePattern {
    pub name: &'static str,
    pub passes: &'static [PassFill],
}

impl WipePattern {
    pub const ZERO: WipePattern = WipePattern {
        name: "Zero (1-pass)",
        passes: &[PassFill::Zero],
    };
    pub const RANDOM: WipePattern = WipePattern {
        name: "Random (1-pass)",
        passes: &[PassFill::Random],
    };
    /// The short variant of DoD 5220.22-M: zero, then all-ones, then
    /// random. See module docs on why this isn't necessarily "more
    /// secure" than a single random pass on modern media — included for
    /// familiarity, not because it's the recommended default.
    pub const DOD_3PASS: WipePattern = WipePattern {
        name: "DoD 5220.22-M (3-pass)",
        passes: &[PassFill::Zero, PassFill::One, PassFill::Random],
    };
}

/// A small, fast, NOT cryptographically-secure PRNG (xorshift64star).
/// That distinction matters and is worth being explicit about: this is
/// fine for "make old bytes disappear from casual/free-space recovery
/// tools like our own carver," but a product that needs to defend
/// against a well-resourced forensic adversary should seed passes from
/// the OS's CSPRNG (e.g. the `rand`/`getrandom` crates) instead — noted
/// here rather than silently implied to be equivalent.
pub struct WipeRng {
    state: u64,
}

impl WipeRng {
    pub fn seeded_from_time() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15);
        let salt = COUNTER.fetch_add(1, Ordering::Relaxed);
        let seed = nanos ^ salt.wrapping_mul(0x2545F4914F6CDD1D);
        Self { state: if seed == 0 { 0xDEAD_BEEF_CAFE_F00D } else { seed } }
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64star
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut i = 0;
        while i < buf.len() {
            let bytes = self.next_u64().to_le_bytes();
            let take = (buf.len() - i).min(8);
            buf[i..i + take].copy_from_slice(&bytes[..take]);
            i += take;
        }
    }
}

/// Fills `buf` according to `pass`. `rng` is threaded through explicitly
/// (rather than each call creating its own) so a caller doing many
/// chunked writes across one pass gets a continuous stream of random
/// bytes, not the same few values repeated every chunk.
pub fn fill_pass(pass: PassFill, buf: &mut [u8], rng: &mut WipeRng) {
    match pass {
        PassFill::Zero => buf.fill(0x00),
        PassFill::One => buf.fill(0xFF),
        PassFill::Random => rng.fill_bytes(buf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_one_fill_exactly() {
        let mut rng = WipeRng::seeded_from_time();
        let mut buf = vec![0xAAu8; 16];

        fill_pass(PassFill::Zero, &mut buf, &mut rng);
        assert!(buf.iter().all(|&b| b == 0x00));

        fill_pass(PassFill::One, &mut buf, &mut rng);
        assert!(buf.iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn random_fill_is_not_constant_and_changes_between_calls() {
        let mut rng = WipeRng::seeded_from_time();
        let mut buf1 = vec![0u8; 64];
        let mut buf2 = vec![0u8; 64];

        fill_pass(PassFill::Random, &mut buf1, &mut rng);
        fill_pass(PassFill::Random, &mut buf2, &mut rng);

        assert!(!buf1.iter().all(|&b| b == buf1[0]), "random fill should not be a constant byte");
        assert_ne!(buf1, buf2, "consecutive random passes from the same rng should differ");
    }

    #[test]
    fn dod_3pass_has_expected_pass_sequence() {
        assert_eq!(WipePattern::DOD_3PASS.passes, &[PassFill::Zero, PassFill::One, PassFill::Random]);
    }
}
