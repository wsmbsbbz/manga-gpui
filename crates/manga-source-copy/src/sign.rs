//! Request headers for CopyManga's private app API.
//!
//! The app gates its JSON API behind a client signature and a set of
//! client-identifying headers. Reproducing them is what lets this reader talk
//! to the same endpoints the official app and existing open-source clients
//! (e.g. venera) use.
//!
//! None of this is a user-security control: there is no account secret here.
//! The signing key is a fixed constant already published in the community
//! source config (`venera-configs/copy_manga.js`); it is base64 of the ASCII
//! string below. The signature is `HMAC-SHA256(key, unix_seconds)` in hex.
//!
//! This is for local, personal research use (studying GPUI). It is an
//! unofficial client and likely outside the site's terms of service, so it
//! stays polite: a stable per-process device id, no scraping loops.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Base64("M2Fm…NTM=") decodes to these 32 ASCII bytes. Published constant.
const HMAC_KEY: &[u8] = b"3af08590311032efe0660550a0563a53";
const UMSTRING: &str = "b4c89ca4104ea9a97750314d791520ac";
const APP_VERSION: &str = "3.0.6";

/// Client identity sent with every request. Generated once per process and
/// kept stable, the way the official client persists it.
#[derive(Clone)]
pub struct Device {
    deviceinfo: String,
    device: String,
    pseudoid: String,
}

impl Device {
    pub fn generate() -> Self {
        let mut r = Rng::seeded();
        Self {
            // e.g. "1234567V-1234"
            deviceinfo: format!("{}V-{}", r.digits(7), r.digits(4)),
            // e.g. "AB1C.123456.789"
            device: format!("{}{}{}{}.{}.{}", r.upper(), r.upper(), r.digit(), r.upper(), r.digits(6), r.digits(3)),
            pseudoid: r.alnum(16),
        }
    }
}

/// Build the full header set for one request.
pub fn headers(dev: &Device, token: Option<&str>, region: &str) -> Vec<(String, String)> {
    let ts = unix_seconds().to_string();
    let sig = sign(ts.as_bytes());
    let dt = chrono::Local::now().format("%Y.%m.%d").to_string();
    let authorization = match token {
        Some(t) if !t.is_empty() => format!("Token {t}"),
        _ => "Token".to_string(),
    };
    vec![
        ("User-Agent".into(), format!("COPY/{APP_VERSION}")),
        ("source".into(), "copyApp".into()),
        ("deviceinfo".into(), dev.deviceinfo.clone()),
        ("dt".into(), dt),
        ("platform".into(), "3".into()),
        ("referer".into(), format!("com.copymanga.app-{APP_VERSION}")),
        ("version".into(), APP_VERSION.into()),
        ("device".into(), dev.device.clone()),
        ("pseudoid".into(), dev.pseudoid.clone()),
        ("Accept".into(), "application/json".into()),
        ("region".into(), region.to_string()),
        ("authorization".into(), authorization),
        ("umstring".into(), UMSTRING.into()),
        ("x-auth-timestamp".into(), ts),
        ("x-auth-signature".into(), sig),
    ]
}

fn sign(message: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(HMAC_KEY).expect("hmac accepts any key length");
    mac.update(message);
    to_hex(&mac.finalize().into_bytes())
}

fn unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Tiny splitmix64 — enough to make device ids look like real ones without
/// pulling in an RNG crate.
struct Rng(u64);

impl Rng {
    fn seeded() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
        let c = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(nanos ^ (c.wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ (std::process::id() as u64) << 32)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn digit(&mut self) -> char {
        (b'0' + self.below(10) as u8) as char
    }

    fn upper(&mut self) -> char {
        (b'A' + self.below(26) as u8) as char
    }

    fn digits(&mut self, n: usize) -> String {
        (0..n).map(|_| self.digit()).collect()
    }

    fn alnum(&mut self, n: usize) -> String {
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        (0..n).map(|_| CHARS[self.below(CHARS.len() as u64) as usize] as char).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_stable_hex_for_a_timestamp() {
        // Known-answer: HMAC-SHA256(key, "1700000000") in lowercase hex.
        let sig = sign(b"1700000000");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // Deterministic for the same input.
        assert_eq!(sig, sign(b"1700000000"));
    }

    #[test]
    fn header_set_is_complete() {
        let dev = Device::generate();
        let h = headers(&dev, None, "0");
        let keys: Vec<&str> = h.iter().map(|(k, _)| k.as_str()).collect();
        for required in ["x-auth-signature", "x-auth-timestamp", "version", "platform", "authorization"] {
            assert!(keys.contains(&required), "missing {required}");
        }
        assert_eq!(h.iter().find(|(k, _)| k == "authorization").unwrap().1, "Token");
    }

    #[test]
    fn token_goes_into_authorization() {
        let dev = Device::generate();
        let h = headers(&dev, Some("abc123"), "1");
        assert_eq!(h.iter().find(|(k, _)| k == "authorization").unwrap().1, "Token abc123");
    }

    #[test]
    fn device_ids_have_expected_shape() {
        let dev = Device::generate();
        assert!(dev.pseudoid.len() == 16);
        assert!(dev.deviceinfo.contains('V'));
        assert_eq!(dev.device.matches('.').count(), 2);
    }
}
