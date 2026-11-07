//! The sealed push payload against the shared test vectors (the phone app uses the same file).

use aivory_link::push::{Envelope, PushPlain, open, seal_with};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;

fn bytes<const N: usize>(v: &Value, key: &str) -> [u8; N] {
    URL_SAFE_NO_PAD
        .decode(v[key].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap()
}

fn vectors() -> Value {
    serde_json::from_str(include_str!("data/push-vectors.json")).unwrap()
}

#[test]
fn seal_matches_the_vector() {
    let v = vectors();
    let env = seal_with(
        &bytes(&v, "device_public"),
        bytes(&v, "ephemeral_secret"),
        bytes(&v, "nonce"),
        v["plaintext"].as_str().unwrap().as_bytes(),
    )
    .unwrap();
    let expected: Envelope = serde_json::from_value(v["envelope"].clone()).unwrap();
    assert_eq!(env, expected);
}

#[test]
fn open_reads_the_vector() {
    let v = vectors();
    let env: Envelope = serde_json::from_value(v["envelope"].clone()).unwrap();
    let plain = open(&bytes(&v, "device_secret"), &env).unwrap();
    let expected: PushPlain = serde_json::from_str(v["plaintext"].as_str().unwrap()).unwrap();
    assert_eq!(plain, expected);
}
