//! Two-factor codes (TOTP, RFC 6238): what authenticator apps make.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const SECRET_BYTES: usize = 20;
const STEP_SECS: u64 = 30;
/// A code is good one step either side, for clocks that are a little off.
const DRIFT_STEPS: u64 = 1;

/// A new secret, in the base32 form authenticator apps take.
pub fn new_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::fill(&mut bytes);
    encode(&bytes)
}

fn encode(bytes: &[u8]) -> String {
    let (mut out, mut buffer, mut bits) = (String::new(), 0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[(buffer >> bits) as usize & 31] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[(buffer << (5 - bits)) as usize & 31] as char);
    }
    out
}

fn decode(text: &str) -> Option<Vec<u8>> {
    let (mut out, mut buffer, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in text.bytes().filter(|c| *c != b'=') {
        let value = ALPHABET.iter().position(|a| *a == c.to_ascii_uppercase())?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
        buffer &= 0xFFFF;
    }
    Some(out)
}

fn code_at(key: &[u8], step: u64) -> Option<u32> {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).ok()?;
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0F) as usize;
    let word = u32::from_be_bytes(digest[offset..offset + 4].try_into().ok()?);
    Some((word & 0x7FFF_FFFF) % 1_000_000)
}

/// Whether `code` is right at `now_secs`.
pub fn check(secret: &str, code: &str, now_secs: u64) -> bool {
    let code = code.trim().replace(' ', "");
    let (Some(key), Ok(code)) = (decode(secret), code.parse::<u32>()) else {
        return false;
    };
    let step = now_secs / STEP_SECS;
    (step.saturating_sub(DRIFT_STEPS)..=step + DRIFT_STEPS)
        .any(|at| code_at(&key, at) == Some(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_rfc_vector() {
        // RFC 6238 appendix B, SHA-1, 6 digits of 94287082.
        let secret = encode(b"12345678901234567890");
        assert_eq!(secret, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert!(check(&secret, "287082", 59));
        assert!(!check(&secret, "287083", 59));
    }

    #[test]
    fn allows_a_little_drift_and_no_more() {
        let secret = encode(b"12345678901234567890");
        assert!(check(&secret, "287 082", 59 + 30));
        assert!(!check(&secret, "287082", 59 + 120));
    }

    #[test]
    fn secrets_survive_the_round_trip() {
        let secret = new_secret();
        assert_eq!(decode(&secret).unwrap().len(), SECRET_BYTES);
    }
}
