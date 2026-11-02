use hmac::{Hmac, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

/// Spotify ships a list of versioned TOTP secrets. Each entry is an XOR-ciphered
/// byte array; the real secret is recovered by XORing byte `i` with `(i % 33) + 9`,
/// decimal-concatenating the result, hex-decoding it and base32-encoding that.
pub fn totp_secret_b32(cipher: &[u8]) -> String {
    let transformed: Vec<u8> = cipher
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ (((i % 33) + 9) as u8))
        .collect();
    let decimal: String = transformed.iter().map(|b| b.to_string()).collect();
    base32_encode(decimal.as_bytes())
}

/// 6-digit HMAC-SHA1 HOTP/TOTP over a 30 second window.
pub fn totp_code(secret_b32: &str, unix_seconds: u64) -> String {
    let key = base32_decode(secret_b32);
    let counter = unix_seconds / 30;
    let mut mac = HmacSha1::new_from_slice(&key).expect("hmac accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let value = u32::from_be_bytes([
        digest[offset],
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]) & 0x7fff_ffff;
    format!("{:06}", value % 1_000_000)
}

const B32: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, unpadded (what `b32encode(...).rstrip("=")` produces).
fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);
        let bits = ((buf[0] as u64) << 32)
            | ((buf[1] as u64) << 24)
            | ((buf[2] as u64) << 16)
            | ((buf[3] as u64) << 8)
            | buf[4] as u64;
        let chars = match chunk.len() {
            1 => 2,
            2 => 4,
            3 => 5,
            4 => 7,
            _ => 8,
        };
        for i in 0..chars {
            out.push(B32[((bits >> (35 - i * 5)) & 0x1f) as usize] as char);
        }
    }
    out
}

fn base32_decode(s: &str) -> Vec<u8> {
    let mut bits = 0u64;
    let mut have = 0u32;
    let mut out = Vec::new();
    for c in s.bytes() {
        let Some(v) = B32.iter().position(|&b| b == c) else {
            continue;
        };
        bits = (bits << 5) | v as u64;
        have += 5;
        if have >= 8 {
            have -= 8;
            out.push((bits >> have) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // secret version 61 as published by Spotify's web player
    const V61: [u8; 26] = [
        44, 55, 47, 42, 70, 40, 34, 114, 76, 74, 50, 111, 120, 97, 75, 76, 94, 102, 43, 69, 49,
        120, 118, 80, 64, 78,
    ];

    #[test]
    fn secret_decoding_matches_known_vector() {
        assert_eq!(
            totp_secret_b32(&V61),
            "GM3TMMJTGYZTQNZVGM4DINJZHA4TGOBYGMZTCMRTGEYDSMJRHE4TEOBUG4YTCMRUGQ4DQOJUGQYTAMRRGA2TCMJSHE3TCMBY"
        );
    }

    #[test]
    fn code_matches_known_vector() {
        assert_eq!(totp_code(&totp_secret_b32(&V61), 1_700_000_000), "371599");
    }
}
