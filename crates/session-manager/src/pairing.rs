//! Host-side pairing code lifecycle: short-lived, single-use, with a cap on
//! failed proofs per code.

use std::time::{Duration, Instant};

use mist_protocol::pairing::PairingCode;

pub const DEFAULT_CODE_TTL: Duration = Duration::from_secs(300);
const MAX_FAILED_PROOFS: u32 = 5;

#[derive(Debug)]
struct ActiveCode {
    code: PairingCode,
    lookup: String,
    expires: Instant,
    failed: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairingCheck {
    /// Proof valid; the code is consumed and must be presented to the user.
    Valid(PairingCode),
    UnknownOrExpired,
    BadProof,
}

#[derive(Debug, Default)]
pub struct PairingCodes {
    active: Vec<ActiveCode>,
}

impl PairingCodes {
    /// Generates a code; any previous code is revoked (one at a time).
    pub fn generate(&mut self, ttl: Duration) -> (PairingCode, String, Instant) {
        self.generate_at(ttl, Instant::now())
    }

    pub fn generate_at(&mut self, ttl: Duration, now: Instant) -> (PairingCode, String, Instant) {
        self.active.clear();
        let code = PairingCode::generate();
        let lookup = code.lookup();
        let expires = now + ttl;
        self.active.push(ActiveCode { code: code.clone(), lookup: lookup.clone(), expires, failed: 0 });
        (code, lookup, expires)
    }

    pub fn cancel(&mut self) -> Option<String> {
        self.active.pop().map(|a| a.lookup)
    }

    pub fn current(&self) -> Option<(&PairingCode, Instant)> {
        self.active.last().map(|a| (&a.code, a.expires))
    }

    /// Verify a request's proof against the code registered under `lookup`.
    pub fn check(&mut self, lookup: &str, client_public_key: &str, nonce: &str, proof: &str) -> PairingCheck {
        self.check_at(lookup, client_public_key, nonce, proof, Instant::now())
    }

    pub fn check_at(&mut self, lookup: &str, client_public_key: &str, nonce: &str, proof: &str, now: Instant) -> PairingCheck {
        self.active.retain(|a| a.expires > now);
        let Some(idx) = self.active.iter().position(|a| a.lookup == lookup) else {
            return PairingCheck::UnknownOrExpired;
        };
        if self.active[idx].code.verify_request_proof(client_public_key, nonce, proof) {
            let a = self.active.remove(idx);
            PairingCheck::Valid(a.code)
        } else {
            self.active[idx].failed += 1;
            if self.active[idx].failed >= MAX_FAILED_PROOFS {
                tracing::warn!("pairing code revoked after repeated invalid proofs");
                self.active.remove(idx);
            }
            PairingCheck::BadProof
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_proof_consumes_code() {
        let mut p = PairingCodes::default();
        let (code, lookup, _) = p.generate(DEFAULT_CODE_TTL);
        let proof = code.request_proof("pk", "n");
        assert_eq!(p.check(&lookup, "pk", "n", &proof), PairingCheck::Valid(code));
        // Single use: replay of the same request fails.
        assert_eq!(p.check(&lookup, "pk", "n", &proof), PairingCheck::UnknownOrExpired);
    }

    #[test]
    fn expired_code_rejected() {
        let mut p = PairingCodes::default();
        let t0 = Instant::now();
        let (code, lookup, _) = p.generate_at(Duration::from_secs(60), t0);
        let proof = code.request_proof("pk", "n");
        assert_eq!(p.check_at(&lookup, "pk", "n", &proof, t0 + Duration::from_secs(61)), PairingCheck::UnknownOrExpired);
    }

    #[test]
    fn repeated_bad_proofs_revoke_code() {
        let mut p = PairingCodes::default();
        let (code, lookup, _) = p.generate(DEFAULT_CODE_TTL);
        for _ in 0..MAX_FAILED_PROOFS {
            assert_eq!(p.check(&lookup, "pk", "n", "00"), PairingCheck::BadProof);
        }
        let proof = code.request_proof("pk", "n");
        assert_eq!(p.check(&lookup, "pk", "n", &proof), PairingCheck::UnknownOrExpired);
    }

    #[test]
    fn new_code_replaces_old() {
        let mut p = PairingCodes::default();
        let (old, old_lookup, _) = p.generate(DEFAULT_CODE_TTL);
        let (_new, _, _) = p.generate(DEFAULT_CODE_TTL);
        let proof = old.request_proof("pk", "n");
        assert_eq!(p.check(&old_lookup, "pk", "n", &proof), PairingCheck::UnknownOrExpired);
    }
}
