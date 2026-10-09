# Security model

Remote input is privileged access to a computer. Mist is designed so that
only devices the host user explicitly paired, and only sessions the host user
explicitly allowed, can see the screen or send input.

## Assets and threats

| Asset | Threat | Mitigation |
|---|---|---|
| Host screen, audio, input | Unpaired device connects | Sessions are accepted only from devices in the host's trust store; every session needs host consent (unless the host user enabled "trust paired devices") |
| Pairing | Code guessing | 40-bit random codes, 5 min TTL, single use, revoked after 5 bad proofs, signaling server rate-limits pair attempts (5/min per connection) and the host user must approve every pairing |
| Pairing | Malicious/compromised signaling server substitutes keys | Both sides exchange HMAC proofs keyed by the code that bind their public keys; the server only knows a SHA-256 *lookup* of the code |
| Established pairing | Malicious signaling server MITM | All peer messages (incl. SDP with DTLS fingerprints, ICE candidates) are signed with Ed25519 device keys and verified against the stored key |
| Media/input on the wire | Eavesdropping/tampering | WebRTC DTLS-SRTP (media) and DTLS-SCTP (data channels); signaling over TLS |
| Replay | Re-sent signed messages | Per-sender nonce cache + ±5 min timestamp window; pairing codes single use |
| Input path | Malformed or abusive messages | Strict JSON schemas (`deny_unknown_fields`), semantic validation, size limits (2 KiB), per-class token-bucket rate limits, no command or shell messages exist in the protocol |
| Stuck input | Disconnect while keys are held | Host releases all keys, buttons and gamepad inputs on disconnect, on 3 s of silence, on permission revocation and on client focus loss |
| Host user control | Need to stop remote control now | "Allow input" switch (immediate, releases held input) and "Disconnect everyone" in the UI; `input off` / `kick` in the CLI |
| Device keys / tokens at rest | Theft by other local users | Unix: files 0600 in a 0700 directory; Windows: DPAPI (CurrentUser) encrypted |
| Signaling server | Abuse / resource exhaustion | Challenge-response auth, optional access token, per-IP and global connection caps, message size limit (64 KiB), per-connection rate limit, auth timeout 10 s, idle timeout 90 s, connection lifetime 24 h |
| TURN relay | Open relay abuse | Ephemeral HMAC credentials (TURN REST API, default 1 h) issued only to authenticated devices; coturn config denies relaying to private address ranges and sets quotas |

## Identities

Each installation generates an Ed25519 key pair (OS RNG via `getrandom`,
`ed25519-dalek`). The **device id** is the first 128 bits of SHA-256 of the
public key, so it cannot be claimed without the private key. A short
**fingerprint** (first 16 hex chars, grouped) is shown in consent prompts.

## Signaling authentication

1. Server sends `challenge { nonce }` (256-bit random).
2. Device sends `hello { device_id, public_key, role, access_token? }` and
   `auth_response { signature }` over `"mist-signal-auth-v1\n" || nonce || device_id`.
3. Server checks that `device_id` matches the public key and verifies the
   signature, then issues STUN/TURN configuration.

The server authenticates *which key* is connected; it does not decide who
may talk to whom. Authorization is end to end (below).

## Pairing protocol

```
Host:   code = 8 Crockford base32 chars (40 bits)      shown on screen
        lookup = SHA-256("mist-pair-lookup-v1\n" || code) -> server (register, TTL ≤ 15 min)
Client: nonce = 128-bit random
        proof_c = HMAC-SHA256(code, "mist-pair-request-v1", client_pk, nonce)
        PairRequest{client_name, client_pk, nonce, proof_c}  signed by client key, routed by lookup
Host:   verify signature, client_pk == authenticated key, proof_c   (single use; 5 failures revoke the code)
        -> show "<name> (fingerprint) wants to pair"  -> user approves
        proof_h = HMAC-SHA256(code, "mist-pair-accept-v1", host_pk, client_pk, nonce)
        PairAccepted{host_name, host_pk, proof_h} signed by host key
Client: verify signature, host_pk == authenticated key, proof_h -> store host key
```

Both sides store only the peer's public key, name and signaling URL.

**Known limitation**: a malicious signaling server knows `lookup` and could
brute-force the 40-bit code offline in its 5-minute validity, then attempt
to pair as a client. The host user would still see an unexpected pairing
request (with a fingerprint) and must approve it; users are told to approve
only pairings they started. A PAKE-based exchange would remove this; it is
listed in TASKS.md.

## Sessions

* The client sends a signed `SessionRequest`. The host verifies the signature
  and that the sender is in its trust store with the **same key** (a key
  mismatch is rejected and logged). Unpaired devices are rejected without
  prompting the user (tested).
* The host user approves or declines (`session_request` event / prompt).
  Approval timeout: 60 s. Session lifetime: 12 h.
* SDP offer/answer and ICE candidates are signed envelopes, so the DTLS
  certificate fingerprints in the SDP are authenticated with device keys.
* Resume after a network failure is accepted without a new prompt only for
  the same session id from the same authenticated client, while the session
  is live or within 60 s after it failed.

## Input handling on the host

`input-api::dispatcher::InputDispatcher`:

* rejects anything that is not a known, valid message (unknown variants and
  unknown fields fail to parse; key codes must be in the HID table; ranges
  for motion/scroll/gamepad are checked);
* drops everything while input is not allowed (except "release all");
* rate limits per class (keyboard 60/s burst 120, mouse 2000/s, gamepads 1200/s);
* tracks held keys/buttons and gamepad state and releases them on
  `release_all`, permission change, interruption and session end;
* ignores stale gamepad packets (sequence numbers with wraparound).

Injection backends only see validated events. There is no message type for
running commands, launching programs or accessing files.

## Privileges

The application never asks for administrator/root rights.

* **Linux X11**: XTEST works for the user's own X session.
* **Linux Wayland / gamepads**: need write access to `/dev/uinput`. Grant it
  narrowly with the udev rule in `deploy/linux/99-mist-uinput.rules`
  (group `mist-input`). Note that any process of a user in that group can
  create input devices.
* **Wayland capture** goes through xdg-desktop-portal, so the compositor
  asks the user which screen to share.
* **Windows**: SendInput cannot inject into windows of higher integrity
  (elevated apps, UAC prompts, the secure desktop) when Mist runs as a normal
  user (UIPI). This is intentional; running the host elevated is not
  recommended. Virtual gamepads need the ViGEmBus driver installed by an
  administrator once.

## Data the signaling server sees

Device ids, public keys, device names, roles, IP addresses, connection times,
pairing lookups, and opaque signed envelopes (which include SDP and ICE
candidates, i.e. IP addresses and DTLS fingerprints, but no media, input or
secrets). Logs do not include envelope contents or tokens.

## Secrets in this repository

None. TURN secrets and access tokens are supplied through environment
variables (`deploy/examples/env.example`) or the app's secrets file. The
test harness generates throwaway CAs and TURN secrets at run time.
