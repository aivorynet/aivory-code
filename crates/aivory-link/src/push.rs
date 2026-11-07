//! Sealed push payload, contract v1 (ADR 0015).
//!
//! The daemon seals a short event for one phone. The push gateway forwards the envelope to
//! APNs or FCM and cannot open it. The phone opens it with its push secret key.
//!
//! Scheme: X25519 ephemeral-static ECDH with the device push public key, HKDF-SHA256 (salt
//! `epk || device_pk`, info `aivory-push-v1`), ChaCha20-Poly1305, no AAD. All binary values
//! are base64url without padding. The test vectors are in `tests/data/push-vectors.json`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

/// Envelope version.
pub const VERSION: u8 = 1;
/// HKDF info string.
pub const INFO: &[u8] = b"aivory-push-v1";
/// Maximum length of `text` in characters.
pub const MAX_TEXT_CHARS: usize = 120;
/// Maximum size of the decoded `ct` field in bytes (the gateway rejects more).
pub const MAX_CT_BYTES: usize = 2048;
const TAG_BYTES: usize = 16;

/// Error of the sealed payload functions.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SealError {
    #[error("unsupported envelope version")]
    Version,
    #[error("a field is not valid base64url")]
    Base64,
    #[error("a field has the wrong length")]
    Length,
    #[error("text is longer than {MAX_TEXT_CHARS} characters")]
    TextTooLong,
    #[error("the public key is a low-order point")]
    WeakKey,
    #[error("decryption failed")]
    Decrypt,
    #[error("the plaintext is not valid JSON of the expected shape")]
    Plaintext,
}

/// The plaintext that the daemon seals and the phone opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushPlain {
    /// Session id.
    pub session: String,
    /// Event kind, for example `approval`, `error`, `review`.
    pub kind: String,
    /// Short text, at most 120 characters. Never transcript content.
    pub text: String,
    /// Unix time in seconds.
    pub ts: u64,
}

impl PushPlain {
    /// Check the length limit of `text`.
    pub fn validate(&self) -> Result<(), SealError> {
        if self.text.chars().count() > MAX_TEXT_CHARS {
            return Err(SealError::TextTooLong);
        }
        Ok(())
    }
}

/// The envelope that travels through the gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    /// Ephemeral public key, 32 bytes.
    pub epk: String,
    /// Nonce, 12 bytes.
    pub nonce: String,
    /// Ciphertext with the 16-byte tag.
    pub ct: String,
}

impl Envelope {
    /// Check the shape without a key.
    pub fn check_shape(&self) -> Result<(), SealError> {
        if self.v != VERSION {
            return Err(SealError::Version);
        }
        if decode(&self.epk)?.len() != 32 || decode(&self.nonce)?.len() != 12 {
            return Err(SealError::Length);
        }
        let ct = decode(&self.ct)?;
        if ct.len() < TAG_BYTES || ct.len() > MAX_CT_BYTES {
            return Err(SealError::Length);
        }
        Ok(())
    }
}

fn decode(s: &str) -> Result<Vec<u8>, SealError> {
    URL_SAFE_NO_PAD.decode(s).map_err(|_| SealError::Base64)
}

fn encode(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

fn array<const N: usize>(s: &str) -> Result<[u8; N], SealError> {
    decode(s)?.try_into().map_err(|_| SealError::Length)
}

/// Decode a device push public key (base64url, 32 bytes) and reject a low-order point.
pub fn parse_public_key(text: &str) -> Result<[u8; 32], SealError> {
    let key: [u8; 32] = array(text)?;
    // A low-order key gives an all-zero shared secret with any private key.
    let probe = StaticSecret::from([7u8; 32]);
    if !probe
        .diffie_hellman(&PublicKey::from(key))
        .was_contributory()
    {
        return Err(SealError::WeakKey);
    }
    Ok(key)
}

/// Make a device key pair. Returns `(secret, public)`. The phone keeps the secret.
pub fn generate_keypair() -> ([u8; 32], [u8; 32]) {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (secret.to_bytes(), public.to_bytes())
}

/// Encode a key or a binary value as base64url without padding.
pub fn to_base64url(bytes: &[u8]) -> String {
    encode(bytes)
}

fn derive_cipher(
    own_secret: &StaticSecret,
    peer_public: &PublicKey,
    epk: &[u8; 32],
    device_pk: &[u8; 32],
) -> Result<ChaCha20Poly1305, SealError> {
    let shared = own_secret.diffie_hellman(peer_public);
    if !shared.was_contributory() {
        return Err(SealError::WeakKey);
    }
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(epk);
    salt[32..].copy_from_slice(device_pk);
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(INFO, &mut key).map_err(|_| SealError::Length)?;
    Ok(ChaCha20Poly1305::new((&key).into()))
}

/// Seal a plaintext for a device. Uses a fresh ephemeral key and a random nonce.
pub fn seal(device_public: &[u8; 32], plain: &PushPlain) -> Result<Envelope, SealError> {
    plain.validate()?;
    let json = serde_json::to_vec(plain).map_err(|_| SealError::Plaintext)?;
    let mut esk = [0u8; 32];
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut esk);
    OsRng.fill_bytes(&mut nonce);
    seal_with(device_public, esk, nonce, &json)
}

/// Seal with a given ephemeral secret and nonce. Use it for test vectors only.
pub fn seal_with(
    device_public: &[u8; 32],
    ephemeral_secret: [u8; 32],
    nonce: [u8; 12],
    plaintext: &[u8],
) -> Result<Envelope, SealError> {
    let esk = StaticSecret::from(ephemeral_secret);
    let epk = PublicKey::from(&esk).to_bytes();
    let cipher = derive_cipher(&esk, &PublicKey::from(*device_public), &epk, device_public)?;
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| SealError::Decrypt)?;
    Ok(Envelope {
        v: VERSION,
        epk: encode(&epk),
        nonce: encode(&nonce),
        ct: encode(&ct),
    })
}

/// Open an envelope and parse the plaintext. The phone does this; the daemon uses it in tests.
pub fn open(device_secret: &[u8; 32], env: &Envelope) -> Result<PushPlain, SealError> {
    env.check_shape()?;
    let epk: [u8; 32] = array(&env.epk)?;
    let nonce: [u8; 12] = array(&env.nonce)?;
    let ct = decode(&env.ct)?;
    let sk = StaticSecret::from(*device_secret);
    let pk = PublicKey::from(&sk).to_bytes();
    let cipher = derive_cipher(&sk, &PublicKey::from(epk), &epk, &pk)?;
    let bytes = cipher
        .decrypt(Nonce::from_slice(&nonce), ct.as_slice())
        .map_err(|_| SealError::Decrypt)?;
    serde_json::from_slice(&bytes).map_err(|_| SealError::Plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> PushPlain {
        PushPlain {
            session: "s-123".into(),
            kind: "approval".into(),
            text: "Allow cargo test?".into(),
            ts: 1_760_000_000,
        }
    }

    #[test]
    fn round_trip() {
        let (sk, pk) = generate_keypair();
        let env = seal(&pk, &plain()).unwrap();
        env.check_shape().unwrap();
        assert_eq!(open(&sk, &env).unwrap(), plain());
    }

    #[test]
    fn two_seals_differ() {
        let (_, pk) = generate_keypair();
        let a = seal(&pk, &plain()).unwrap();
        let b = seal(&pk, &plain()).unwrap();
        assert_ne!(a.epk, b.epk);
        assert_ne!(a.ct, b.ct);
    }

    #[test]
    fn wrong_key_and_tampering_fail() {
        let (_, pk) = generate_keypair();
        let (other, _) = generate_keypair();
        let env = seal(&pk, &plain()).unwrap();
        assert_eq!(open(&other, &env), Err(SealError::Decrypt));
        let (sk, pk) = generate_keypair();
        let mut env = seal(&pk, &plain()).unwrap();
        let mut ct = decode(&env.ct).unwrap();
        ct[0] ^= 1;
        env.ct = encode(&ct);
        assert_eq!(open(&sk, &env), Err(SealError::Decrypt));
    }

    #[test]
    fn text_limit_counts_characters() {
        let (_, pk) = generate_keypair();
        let mut p = plain();
        p.text = "x".repeat(MAX_TEXT_CHARS + 1);
        assert_eq!(seal(&pk, &p), Err(SealError::TextTooLong));
        p.text = "\u{e4}".repeat(MAX_TEXT_CHARS);
        assert!(seal(&pk, &p).is_ok());
    }

    #[test]
    fn low_order_key_is_rejected() {
        assert_eq!(seal(&[0u8; 32], &plain()).unwrap_err(), SealError::WeakKey);
        assert_eq!(
            parse_public_key(&encode(&[0u8; 32])),
            Err(SealError::WeakKey)
        );
        let (_, pk) = generate_keypair();
        assert_eq!(parse_public_key(&encode(&pk)), Ok(pk));
        assert_eq!(parse_public_key("AAAA"), Err(SealError::Length));
        assert_eq!(parse_public_key("!!!"), Err(SealError::Base64));
    }
}
