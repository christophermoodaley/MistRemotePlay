# Tests

| Layer | Where | How to run |
|---|---|---|
| Unit | `crates/*/src/**` (`#[cfg(test)]`) and `apps/desktop/src/**/*.test.ts` | `cargo test --workspace`, `npm test` in `apps/desktop` |
| Integration (per crate) | `crates/*/tests/*.rs` (WebRTC loopback, signaling, XTest) | `cargo test --workspace` |
| Integration (whole system, in-process) | `tests/integration/*.rs` | `cargo test -p mist-integration-tests` |
| End-to-end (separate processes, real capture/audio/input, netns, TURN) | `tests/e2e/e2e.py` | see `docs/testing.md` |

Unit tests are kept next to the code they test (Rust convention), so there is
no separate `tests/unit` tree.
