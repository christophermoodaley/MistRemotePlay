//! Pairing code primitives.
//!
//! A pairing code is 8 Crockford base32 characters (40 bits from the OS RNG),
//! displayed on the host and typed on the client. The signaling server only
//! learns `lookup = SHA-256(domain || code)` so it can route the request; the
//! code itself keys HMAC proofs that bind each side's public key, so a relay
//! that does not know the code cannot substitute its own keys. The host user
//! must still approve every pairing explicitly (see docs/security.md).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::{invalid, Result};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const CODE_LEN: usize = 8;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode(String);

impl std::fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingCode(<redacted>)")
    }
}

impl PairingCode {
    pub fn generate() -> Self {
        let mut bytes = [0u8; CODE_LEN];
        getrandom::getrandom(&mut bytes).expect("OS random number generator unavailable");
        // 256 is a multiple of 32, so masking keeps the distribution uniform.
        let s = bytes.iter().map(|b| ALPHABET[(b & 31) as usize] as char).collect();
        PairingCode(s)
    }

    /// Accepts user input such as "abcd-efgh", tolerating the usual Crockford
    /// confusions (O->0, I/L->1).
    pub fn parse(input: &str) -> Result<Self> {
        let mut out = String::with_capacity(CODE_LEN);
        for c in input.chars() {
            if c == '-' || c == ' ' {
                continue;
            }
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            if !ALPHABET.contains(&(c as u8)) || !c.is_ascii() {
                return Err(invalid("pairing_code", "contains characters outside the code alphabet"));
            }
            out.push(c);
        }
        if out.len() != CODE_LEN {
            return Err(invalid("pairing_code", format!("must be {CODE_LEN} characters")));
        }
        Ok(PairingCode(out))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Display form "ABCD-EFGH".
    pub fn display(&self) -> String {
        format!("{}-{}", &self.0[..4], &self.0[4..])
    }

    /// Routing handle given to the signaling server.
    pub fn lookup(&self) -> String {
        let mut h = Sha256::new();
        h.update(b"mist-pair-lookup-v1\n");
        h.update(self.0.as_bytes());
        hex::encode(h.finalize())
    }

    fn mac(&self, parts: &[&str]) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(self.0.as_bytes()).expect("HMAC accepts any key length");
        for p in parts {
            mac.update(p.as_bytes());
            mac.update(b"\n");
        }
        mac
    }

    /// Proof sent by the client: binds its public key and nonce to the code.
    pub fn request_proof(&self, client_public_key: &str, nonce: &str) -> String {
        hex::encode(self.mac(&["mist-pair-request-v1", client_public_key, nonce]).finalize().into_bytes())
    }

    pub fn verify_request_proof(&self, client_public_key: &str, nonce: &str, proof: &str) -> bool {
        let Ok(bytes) = hex::decode(proof) else { return false };
        self.mac(&["mist-pair-request-v1", client_public_key, nonce]).verify_slice(&bytes).is_ok()
    }

    /// Proof sent back by the host: binds both public keys to the code.
    pub fn accept_proof(&self, host_public_key: &str, client_public_key: &str, nonce: &str) -> String {
        hex::encode(
            self.mac(&["mist-pair-accept-v1", host_public_key, client_public_key, nonce]).finalize().into_bytes(),
        )
    }

    pub fn verify_accept_proof(
        &self,
        host_public_key: &str,
        client_public_key: &str,
        nonce: &str,
        proof: &str,
    ) -> bool {
        let Ok(bytes) = hex::decode(proof) else { return false };
        self.mac(&["mist-pair-accept-v1", host_public_key, client_public_key, nonce]).verify_slice(&bytes).is_ok()
    }
}

pub fn validate_lookup(lookup: &str) -> Result<()> {
    if lookup.len() != 64 || !lookup.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(invalid("lookup", "must be 64 lowercase hex characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_parse_and_differ() {
        let a = PairingCode::generate();
        let b = PairingCode::generate();
        assert_ne!(a, b);
        assert_eq!(PairingCode::parse(&a.display()).unwrap(), a);
        assert_eq!(PairingCode::parse(&a.as_str().to_lowercase()).unwrap(), a);
        validate_lookup(&a.lookup()).unwrap();
    }

    #[test]
    fn parse_normalises_confusable_characters() {
        assert_eq!(PairingCode::parse("oil0-abcd").unwrap().as_str(), "0110ABCD");
        assert!(PairingCode::parse("ABCD-EFG").is_err());
        assert!(PairingCode::parse("ABCD-EFGU").is_err());
        assert!(PairingCode::parse("ABCDÉFGH").is_err());
    }

    #[test]
    fn proofs_bind_keys_and_code() {
        let code = PairingCode::generate();
        let p = code.request_proof("clientkey", "n1");
        assert!(code.verify_request_proof("clientkey", "n1", &p));
        assert!(!code.verify_request_proof("otherkey", "n1", &p));
        assert!(!code.verify_request_proof("clientkey", "n2", &p));
        assert!(!PairingCode::generate().verify_request_proof("clientkey", "n1", &p));
        let a = code.accept_proof("host", "client", "n1");
        assert!(code.verify_accept_proof("host", "client", "n1", &a));
        assert!(!code.verify_accept_proof("evil", "client", "n1", &a));
        assert!(!code.verify_accept_proof("host", "client", "n1", "zz"));
    }

    #[test]
    fn debug_never_leaks_code() {
        let code = PairingCode::generate();
        assert!(!format!("{code:?}").contains(code.as_str()));
    }
}
