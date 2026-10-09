# Testing

## Quick reference

```bash
cargo test --workspace                          # unit + integration (needs GStreamer)
DISPLAY=:99 cargo test -p mist-input-api        # also runs the XTEST test against an X server
cargo test -p mist-integration-tests            # whole system in one process
(cd apps/desktop && npm test && npx tsc --noEmit)
PKG_CONFIG_ALLOW_CROSS=1 cargo check --target x86_64-pc-windows-gnu -p mist-app-core -p mist-signaling-server
```

## What is covered

**Unit tests** (in each crate):

* protocol: identities and signatures, pairing codes/proofs, envelope signing,
  recipient binding, staleness, replay guard, message validation (unknown
  types/fields, oversize, ranges), HID key table consistency, settings
  validation, rate limiter.
* session-manager: pairing code expiry, single use, failed-proof revocation;
  trust store persistence and tamper rejection; session state machine
  (cannot skip approval, terminal end, approval timeout, expiry); reconnect
  backoff; adaptive bitrate (loss, RTT inflation, decoder drops, bounds).
* input-api: permission gate, release on revocation/disconnect, malformed
  input, flood limiting, stale gamepad packets, autorepeat policy.
* controller-api: axis/trigger precision, radial deadzone, remapping,
  inversion, uinput event diffs, slot assignment with reconnect affinity.
* media-engine: SEI timestamp insertion/extraction incl. emulation prevention,
  PTS map, rate counter, clock-offset estimation, ICE URL conversion, aspect fit.
* signaling-server: TURN REST credential matches an OpenSSL reference.
* app-core: config validation/round trip, secret file permissions, identity
  persistence. UI: presets/validation/formatting (Vitest).

**Integration tests**:

* `crates/media-engine/tests/loopback.rs` — real `webrtcbin` host→client:
  changing video frames, SEI timestamps, audio packets, data channels, stats.
* `crates/signaling-client/tests/signaling.rs` — pairing routing, relay,
  presence, offline peers, forged identity rejected, wrong/missing access
  token, client may not register codes, pairing brute-force rate limit, TLS
  with a custom CA (untrusted cert refused).
* `crates/input-api/tests/x11_xtest.rs` — XTEST events read back by an X client.
* `tests/integration/sessions.rs` — full system in-process: pairing + consent
  + video + input + disconnect cleanup; unpaired device rejected without a
  prompt; declined session never streams; input revocation reported and
  enforced; emergency stop.

**End-to-end** (`tests/e2e/e2e.py`, separate processes; root needed for
network namespaces):

| Command | Checks |
|---|---|
| `DISPLAY=:99 e2e.py` | real X11 capture + PulseAudio monitor → client; frames change, timestamps, audio packets |
| `--netns --tls` | separate network stacks; TLS signaling with a generated CA |
| `--netns --tls --block-direct --turn` | direct path blocked → TURN relay required and used |
| `--netns --tls --block-direct --block-udp --turn --turn-tcp` | all UDP dropped → TURN over TCP |
| `--netns --tls --impair-loss 0.2` | random loss; NACK recovery and bitrate adaptation |
| `--netns --tls --interrupt 13` | network outage mid-session; recovery |
| `--input-check` | client input injected with XTEST into the host display, verified by an X event recorder; held key released on disconnect |
| `--input-check --kill-client` | client SIGKILLed while holding a key; host releases it and ends the session |
| `--window-display :98` | the real streaming window renders the stream (screenshot saved) |
| `--test-pattern` | no X display needed on the host |

## Test environment used for the recorded results

Ubuntu 24.04 VM (4 vCPU, no GPU, no `/dev/uinput`), Xvfb, PulseAudio 16 with
a null sink, coturn 4.6.1. See docs/benchmarks.md for the measured numbers.

## Results of the last full run (2026-10-09)

* `cargo test --workspace` (Xvfb on `:99`): **89 passed, 0 failed, 0 ignored**,
  run after the clippy changes. The in-process `sessions` test was also run
  40 times in a row without a failure after an intermittent hang was fixed.
* `cargo clippy --workspace --all-targets -- -D warnings`: clean.
* `cargo fmt --all --check`, Windows cross-check (`x86_64-pc-windows-gnu`),
  `npx tsc --noEmit` and Vitest (4 passed): clean.
* `.deb` built with `npx tauri build --bundles deb`, installed with `dpkg -i`;
  the installed `mist` CLI and desktop app start (Home screen renders).
* Desktop app: driven with xdotool across two instances (pairing via code
  and LAN discovery, consent prompts, live stats, disconnect-everyone).
* All e2e scenarios in the table above: PASS. Re-run after the clippy changes:
  `--test-pattern` smoke (as in CI) and `--netns --tls --block-direct --turn`
  (relayed path confirmed): PASS.
