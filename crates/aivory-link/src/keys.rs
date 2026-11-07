//! Ed25519 keys as 64-character hex strings. The daemon keeps its secret key in a file. The
//! phone keeps its device key in its own secure storage and passes it to the client.

use std::io::Write;
use std::path::Path;

use iroh::SecretKey;

use crate::LinkError;

/// A new random secret key (hex).
pub fn generate_secret_hex() -> String {
    hex(&SecretKey::generate().to_bytes())
}

/// Parse a secret key from hex.
pub fn parse_secret(text: &str) -> Result<SecretKey, LinkError> {
    text.trim()
        .parse::<SecretKey>()
        .map_err(|_| LinkError::Invalid("the secret key is not 64 hex characters".into()))
}

/// The public key (hex) of a secret key (hex). This is also the iroh node id.
pub fn public_hex(secret_hex: &str) -> Result<String, LinkError> {
    Ok(parse_secret(secret_hex)?.public().to_string())
}

/// Load the secret key from `path`, or create the file with a new key. On Unix the file is
/// readable by the owner only.
pub fn load_or_create_secret(path: &Path) -> std::io::Result<SecretKey> {
    if let Ok(text) = std::fs::read_to_string(path) {
        return parse_secret(&text).map_err(std::io::Error::other);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let key = SecretKey::generate();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(hex(&key.to_bytes()).as_bytes())?;
            Ok(key)
        }
        // Another process created it first: use its key.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            parse_secret(&std::fs::read_to_string(path)?).map_err(std::io::Error::other)
        }
        Err(e) => Err(e),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_and_public_keys_round_trip() {
        let secret = generate_secret_hex();
        assert_eq!(secret.len(), 64);
        let public = public_hex(&secret).unwrap();
        assert_eq!(public.len(), 64);
        assert_ne!(public, secret);
        assert!(public_hex("nope").is_err());
    }

    #[test]
    fn the_key_file_is_created_once() {
        let dir = std::env::temp_dir().join(format!("aivory-link-key-{}", std::process::id()));
        let file = dir.join("iroh.key");
        let _ = std::fs::remove_dir_all(&dir);
        let a = load_or_create_secret(&file).unwrap();
        let b = load_or_create_secret(&file).unwrap();
        assert_eq!(a.public(), b.public());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
