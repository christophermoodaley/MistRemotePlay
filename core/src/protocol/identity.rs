//! Device identities: every installation owns an Ed25519 key pair. The device
//! id is derived from the public key so it cannot be claimed without the key.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

use crate::protocol::{invalid, ProtocolError, Result};

/// 32 lowercase hex characters (first 128 bits of SHA-256 of the public key).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    pub const LEN: usize = 32;

    pub fn from_public_key(key: &VerifyingKey) -> Self {
        let digest = Sha256::digest(key.as_bytes());
        DeviceId(hex::encode(&digest[..16]))
    }

    pub fn parse(s: &str) -> Result<Self> {
        if s.len() != Self::LEN || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(invalid("device_id", "must be 32 lowercase hex characters"));
        }
        Ok(DeviceId(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Human-comparable fingerprint shown during pairing ("ab12-cd34-...").
    pub fn fingerprint(&self) -> String {
        self.0.as_bytes()[..16].chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join("-")
    }

    pub fn validate(&self) -> Result<()> {
        Self::parse(&self.0).map(|_| ())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.0)
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Base64-encoded Ed25519 public key as carried on the wire.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicKeyB64(pub String);

impl PublicKeyB64 {
    pub fn from_key(key: &VerifyingKey) -> Self {
        PublicKeyB64(B64.encode(key.as_bytes()))
    }

    pub fn to_key(&self) -> Result<VerifyingKey> {
        if self.0.len() > 64 {
            return Err(invalid("public_key", "too long"));
        }
        let bytes = B64.decode(&self.0).map_err(|_| invalid("public_key", "not base64"))?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| invalid("public_key", "must be 32 bytes"))?;
        VerifyingKey::from_bytes(&arr).map_err(|_| invalid("public_key", "not a valid Ed25519 point"))
    }

    pub fn device_id(&self) -> Result<DeviceId> {
        Ok(DeviceId::from_public_key(&self.to_key()?))
    }
}

/// The local device's private identity.
pub struct DeviceIdentity {
    key: SigningKey,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let mut secret = [0u8; 32];
        getrandom::getrandom(&mut secret).expect("OS random number generator unavailable");
        let id = DeviceIdentity { key: SigningKey::from_bytes(&secret) };
        secret.fill(0);
        id
    }

    pub fn from_secret_bytes(bytes: &[u8]) -> Result<Self> {
        let arr: [u8; 32] = bytes.try_into().map_err(|_| invalid("secret_key", "must be 32 bytes"))?;
        Ok(DeviceIdentity { key: SigningKey::from_bytes(&arr) })
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    pub fn public_key(&self) -> VerifyingKey {
        self.key.verifying_key()
    }

    pub fn public_key_b64(&self) -> PublicKeyB64 {
        PublicKeyB64::from_key(&self.public_key())
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_public_key(&self.public_key())
    }

    pub fn sign(&self, msg: &[u8]) -> String {
        B64.encode(self.key.sign(msg).to_bytes())
    }
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the secret key.
        write!(f, "DeviceIdentity({})", self.device_id())
    }
}

pub fn verify_signature(key: &VerifyingKey, msg: &[u8], signature_b64: &str) -> Result<()> {
    if signature_b64.len() > 128 {
        return Err(ProtocolError::BadSignature);
    }
    let bytes = B64.decode(signature_b64).map_err(|_| ProtocolError::BadSignature)?;
    let arr: [u8; 64] = bytes.try_into().map_err(|_| ProtocolError::BadSignature)?;
    key.verify(msg, &Signature::from_bytes(&arr)).map_err(|_| ProtocolError::BadSignature)
}

/// Bytes a device signs to authenticate itself to the signaling server.
pub fn server_auth_message(nonce: &str, device_id: &DeviceId) -> Vec<u8> {
    format!("mist-signal-auth-v1\n{nonce}\n{device_id}").into_bytes()
}

/// Cryptographically random token, hex encoded.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("OS random number generator unavailable");
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_bound_to_key() {
        let id = DeviceIdentity::generate();
        let pk = id.public_key_b64();
        assert_eq!(pk.device_id().unwrap(), id.device_id());
        assert_eq!(id.device_id().as_str().len(), 32);
        assert_ne!(DeviceIdentity::generate().device_id(), id.device_id());
    }

    #[test]
    fn signatures_verify_and_reject_tampering() {
        let id = DeviceIdentity::generate();
        let sig = id.sign(b"hello");
        verify_signature(&id.public_key(), b"hello", &sig).unwrap();
        assert_eq!(verify_signature(&id.public_key(), b"hellO", &sig), Err(ProtocolError::BadSignature));
        let other = DeviceIdentity::generate();
        assert_eq!(verify_signature(&other.public_key(), b"hello", &sig), Err(ProtocolError::BadSignature));
        assert_eq!(verify_signature(&id.public_key(), b"hello", "!!"), Err(ProtocolError::BadSignature));
    }

    #[test]
    fn device_id_parse_rejects_garbage() {
        assert!(DeviceId::parse("ABCDEF").is_err());
        assert!(DeviceId::parse(&"g".repeat(32)).is_err());
        assert!(DeviceId::parse(&"a".repeat(32)).is_ok());
    }

    #[test]
    fn identity_roundtrips_secret() {
        let id = DeviceIdentity::generate();
        let again = DeviceIdentity::from_secret_bytes(&id.secret_bytes()).unwrap();
        assert_eq!(again.device_id(), id.device_id());
    }
}
