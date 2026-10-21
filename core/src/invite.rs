//! Pairing invites: everything another PC needs to add this host, in one
//! line that can be copied and pasted (chat, email):
//!
//! `mist://pair/ABCD-1234?name=Chris%20PC&fp=<sha256 hex>&at=192.168.1.33:47900,203.0.113.7:47900`
//!
//! - the pairing code (single use, short-lived, and still approved on the
//!   host),
//! - the host's name (for display only),
//! - the SHA-256 fingerprint of the host's server certificate, so the
//!   client trusts it without the manual fingerprint comparison: the
//!   invite comes from the host itself, like the code,
//! - the host's addresses, local network first.
//!
//! The desktop app builds invites in its UI; this module reads them and
//! pairs with the first address that answers.

use std::time::Duration;

use crate::sessions::trust::PairedDevice;
use crate::AppContext;

const PREFIX: &str = "mist://pair/";
/// How long an address may take to accept a connection before the next
/// one is preferred.
const REACH_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Invite {
    pub code: String,
    pub name: Option<String>,
    /// Lowercase hex SHA-256 of the host's server certificate.
    pub fingerprint: Option<String>,
    /// `host:port` addresses, in order of preference.
    pub addresses: Vec<String>,
}

/// Find an invite anywhere in `text` (people paste whole messages).
pub fn parse(text: &str) -> Option<Invite> {
    let start = text.find(PREFIX)?;
    let rest = &text[start..];
    let end = rest.find(|c: char| c.is_whitespace() || c == '"' || c == '<' || c == '>').unwrap_or(rest.len());
    let u = url::Url::parse(&rest[..end]).ok()?;
    let code = u.path().trim_start_matches('/').to_string();
    crate::protocol::pairing::PairingCode::parse(&code).ok()?;
    let mut inv = Invite { code, name: None, fingerprint: None, addresses: Vec::new() };
    for (k, v) in u.query_pairs() {
        match k.as_ref() {
            "name" => inv.name = Some(v.chars().take(64).collect()),
            "fp" => {
                let fp: String = v.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_lowercase();
                if fp.len() == 64 {
                    inv.fingerprint = Some(fp);
                }
            }
            "at" => {
                inv.addresses.extend(v.split(',').map(str::trim).filter(|a| !a.is_empty()).take(8).map(String::from))
            }
            _ => {}
        }
    }
    Some(inv)
}

/// The first of `addresses` (as signaling URLs) whose server accepts a TCP
/// connection. All are tried at once; earlier ones win when several answer.
pub async fn first_reachable(addresses: &[String]) -> Option<String> {
    let urls: Vec<String> = addresses.iter().filter_map(|a| crate::known_servers::normalize_address(a).ok()).collect();
    let probes = urls.iter().map(|url| async move {
        let u = url::Url::parse(url).ok()?;
        let target = format!("{}:{}", u.host_str()?, u.port_or_known_default()?);
        tokio::time::timeout(REACH_TIMEOUT, tokio::net::TcpStream::connect(target)).await.ok()?.ok().map(|_| ())
    });
    let results = futures::future::join_all(probes).await;
    urls.into_iter().zip(results).find(|(_, ok)| ok.is_some()).map(|(u, _)| u)
}

/// Add the host an invite describes: pick an address that answers, trust
/// its certificate, and pair with the code (the host still approves).
pub async fn pair(ctx: &AppContext, invite: &Invite) -> anyhow::Result<PairedDevice> {
    let mut ctx = ctx.clone();
    if !invite.addresses.is_empty() {
        let url = first_reachable(&invite.addresses).await.ok_or_else(|| {
            anyhow::anyhow!(
                "could not reach {} at {}. Is Mist hosting there, and is the port open?",
                invite.name.as_deref().unwrap_or("the host"),
                invite.addresses.join(" or ")
            )
        })?;
        if let Some(fp) = &invite.fingerprint {
            crate::known_servers::trust(&ctx.dir, &url, fp)?;
        }
        ctx.config.network.signaling_url = url;
    }
    crate::client::pair(&ctx, &invite.code).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_invite_inside_a_message() {
        let fp = "ab".repeat(32);
        let text = format!(
            "Join my PC: mist://pair/ABCD-1234?name=Chris%20PC&fp={}&at=192.168.1.33:47900,203.0.113.7:47900 see you",
            fp.to_uppercase()
        );
        let inv = parse(&text).unwrap();
        assert_eq!(inv.code, "ABCD-1234");
        assert_eq!(inv.name.as_deref(), Some("Chris PC"));
        assert_eq!(inv.fingerprint.as_deref(), Some(fp.as_str()));
        assert_eq!(inv.addresses, vec!["192.168.1.33:47900", "203.0.113.7:47900"]);
    }

    #[test]
    fn parses_what_the_desktop_app_copies() {
        // HostPage.tsx inviteText(): encodeURIComponent on name and addresses.
        let text = "Play on Chris PC with Mist Remote Play: open Mist, choose Add a host, then Paste invite.\n\
                    mist://pair/K7QM-2XRT?name=Chris%20PC&fp=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\
                    &at=192.168.1.33%3A47900%2C155.93.192.77%3A47900";
        let inv = parse(text).unwrap();
        assert_eq!(inv.code, "K7QM-2XRT");
        assert_eq!(inv.name.as_deref(), Some("Chris PC"));
        assert!(inv.fingerprint.is_some());
        assert_eq!(inv.addresses, vec!["192.168.1.33:47900", "155.93.192.77:47900"]);
    }

    #[test]
    fn rejects_bad_codes_and_ignores_bad_fingerprints() {
        assert!(parse("mist://pair/NOPE").is_none());
        assert!(parse("no invite here").is_none());
        let inv = parse("mist://pair/abcd1234?fp=xyz").unwrap();
        assert_eq!(inv.fingerprint, None);
        assert!(inv.addresses.is_empty());
    }
}
