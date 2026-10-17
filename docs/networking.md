# Networking, signaling and TURN

## Connection strategy

1. **Authenticate** both devices to the signaling server (Ed25519 challenge).
2. **Request a session**; the host user approves it.
3. **Exchange SDP and ICE candidates** through the server (signed end to end).
4. **ICE** tries host candidates (LAN), server-reflexive candidates (STUN,
   through NAT) and ICE-TCP, then **relay** candidates (TURN over UDP, TCP or
   TLS) — the first working pair wins.
5. **Detect failures**: ICE state plus application liveness (1 s pings, 3 s
   silence threshold). Held input is released immediately on interruption.
6. **Recover**: wait for the same connection to come back (grace period), then
   build a new peer connection under the same approved session (`resume`).

Port forwarding is not needed in the normal case. Not every network allows a
direct path (e.g. both peers behind symmetric NAT, or UDP blocked); then a
TURN relay is required.

### Relay costs

A relayed session sends every media packet through the TURN server: the
server's bandwidth carries the full stream in and out (≈ 2 × stream bitrate,
e.g. ~30 Mbit/s for a 15 Mbit/s stream), and latency increases by the
detour to the relay. On our local test bridge the relay added no measurable
latency (both ≈ 11 ms at 1080p60), but on the internet expect roughly the
extra round trip to the relay's location. Use **relay-only** mode only when
needed (it also hides your IP address from the peer).

## Built-in server (default)

Every host runs the signaling server itself while hosting, so a normal
install needs no separate server:

* It listens on **TCP 47900** on all interfaces (`host.server_port`;
  `mist host --server-port`), with TLS using a self-signed certificate
  generated on first use (key in owner-only storage, DPAPI on Windows).
* Clients reach it at `wss://<host address>:47900`. Same network: found via
  mDNS, which also carries the certificate fingerprint. Internet: forward
  the TCP port on the host's router and use the public address.
* The first connection to an address shows the certificate's SHA-256
  fingerprint; once the user confirms it matches the host's (Host → How
  others connect), it is pinned in `known-servers.json` and a different
  certificate is refused from then on (`mist pair <code> --host <addr>`
  asks the same question; `--trust <fingerprint>` for scripts).
* The server hands out public STUN servers (Cloudflare, Google) so media can
  cross NATs. It does not provide TURN; when no direct path exists, use a
  standalone server with TURN (below) and turn the built-in one off
  (Settings → Host, or `host.builtin_server = false`). Passing
  `--signaling` to the CLI also uses that server instead.

Trusting a pinned server is low-stakes by design: the signaling server
never sees media or input, pairing needs the host user's approval, and all
messages between devices are signed with their paired keys.

## Signaling server

`mist-signaling-server` is configured entirely by environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `MIST_SIGNAL_BIND` | `127.0.0.1:8443` | Listen address |
| `MIST_TLS_CERT`, `MIST_TLS_KEY` | – | PEM certificate chain and key; enables `wss://` |
| `MIST_ALLOW_INSECURE_WS` | – | `1` permits plain `ws://` on a non-loopback address (only behind a TLS proxy) |
| `MIST_ACCESS_TOKEN` | – | Require this token from every device |
| `MIST_STUN_URLS` | – | Comma-separated `stun:` URLs handed to clients |
| `MIST_TURN_URLS` | – | Comma-separated `turn:`/`turns:` URLs |
| `MIST_TURN_SECRET` | – | coturn `static-auth-secret` (required with TURN URLs) |
| `MIST_TURN_TTL_SECS` | `3600` | Lifetime of issued TURN credentials |
| `MIST_MAX_CONNECTIONS`, `MIST_MAX_CONNECTIONS_PER_IP` | `2000`, `32` | Connection caps |
| `MIST_CONNECTION_TTL_SECS`, `MIST_IDLE_TIMEOUT_SECS` | `86400`, `90` | Connection lifetime / idle timeout |
| `MIST_LOG`, `MIST_LOG_FORMAT` | `info`, text | Log filter; `json` for structured logs |

Endpoints: `GET /ws` (WebSocket), `GET /healthz` (liveness), `GET /readyz`
(JSON: online devices, counters, whether TURN is configured).

The server refuses to start with plain `ws://` on a public address unless
`MIST_ALLOW_INSECURE_WS=1`, and clients refuse `ws://` to anything but
`localhost`.

## TURN with coturn

The signaling server issues **ephemeral credentials** (TURN REST API):
`username = "<expiry>:<device-id>"`, `password = base64(HMAC-SHA1(secret, username))`.
coturn validates them with the same secret (`use-auth-secret`). The secret
never reaches clients.

### Deployment (Docker Compose)

```bash
cd deploy
cp examples/env.example .env       # set MIST_PUBLIC_HOST and MIST_TURN_SECRET (openssl rand -hex 32)
# put a certificate for TURN-over-TLS in deploy/coturn/certs/{fullchain,privkey}.pem
docker compose up -d
```

This runs the signaling server behind Caddy (automatic HTTPS → `wss://`)
and coturn with host networking. Open TCP 80/443, UDP+TCP 3478, TCP 5349 and
UDP 49160–49999. Edit `external-ip` in `coturn/turnserver.conf` if the server
is behind 1:1 NAT (most cloud VMs). The compose file was written for this
project but **has not been run** in the development environment (no Docker
available); the coturn config itself was parsed by coturn 4.6.1 and the TURN
flows were tested with coturn directly (below).

### Without Docker

```bash
turnserver -c deploy/coturn/turnserver.conf --static-auth-secret="$MIST_TURN_SECRET"
MIST_SIGNAL_BIND=0.0.0.0:443 MIST_TLS_CERT=/etc/letsencrypt/live/HOST/fullchain.pem \
MIST_TLS_KEY=/etc/letsencrypt/live/HOST/privkey.pem \
MIST_STUN_URLS=stun:HOST:3478 \
MIST_TURN_URLS="turn:HOST:3478?transport=udp,turn:HOST:3478?transport=tcp,turns:HOST:5349?transport=tcp" \
MIST_TURN_SECRET=... mist-signaling-server
```

A hardened systemd unit is in `deploy/systemd/mist-signaling.service`.

### Configured vs working TURN

`mist ice-check` (Settings → Network → *Test connectivity* in the app)
gathers ICE candidates through the configured servers. A **relay candidate
proves the TURN server accepted the credentials and allocated a relay**; the
command prints `TURN server WORKS`, `configured but NOT working`, or `no TURN
server configured`. The streaming statistics also show whether the active
path is `direct` or `TURN relay`.

## Client configuration

`mist config set-signaling wss://signal.example.org`,
`mist config set-access-token …`, `mist config set-ca ca.pem` (self-hosted
CAs), `mist config add-ice-server turn:… --username … --credential …`,
`mist config set-relay-only true`. The same settings are in the app under
Settings → Network.

## LAN discovery

Hosts with LAN discovery enabled advertise `_mist-play._udp.local.` via mDNS
with their device id, name and either their built-in server's port and
certificate fingerprint or the signaling URL they use. Discovery is a convenience
only; pairing still needs the code and every session is authenticated with
paired keys.

## Tested network scenarios

All with the end-to-end harness (`tests/e2e/e2e.py`), host and client in
separate Linux network namespaces joined by a bridge, signaling over TLS:

| Scenario | Result |
|---|---|
| Direct LAN path | host↔host candidates selected |
| Direct traffic between peers dropped, coturn available | relay candidate selected, stream at 60 fps (1080p) |
| All UDP dropped in both namespaces, TURN over TCP only | relay↔relay selected via TURN/TCP, stream OK (ICE ~2 s slower) |
| Random 5 % / 20 % loss on everything the host sends | NACK recovery; at 20 % the bitrate controller dropped to its floor; stream kept running |
| 10 s total outage on the client | same connection recovered 0.5 s after restore |
| 13 s outage | new peer connection (resume), streaming 1.3 s after restore |
| 25 s outage (host already ended the session) | resume accepted, streaming 10.4 s after restore (signaling backoff) |
| Client killed (SIGKILL) while holding a key | host released the key and ended the session after 18 s |

Not tested: real internet paths, carrier-grade/symmetric NAT on physical
networks, TURN over TLS (`turns:`), IPv6-only networks, Wi-Fi roaming between
networks.
