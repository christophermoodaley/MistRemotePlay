# Relay / signaling server for a Linux VPS (optional)

Only needed when two PCs cannot connect directly over the internet (the
stream never starts). Normally the host PC runs Mist's server itself.

## Option A: Docker Compose (from the source tree)

```bash
cd deploy
cp examples/env.example .env   # set MIST_PUBLIC_HOST and MIST_TURN_SECRET (openssl rand -hex 32)
docker compose up -d           # signaling + coturn (TURN) + Caddy (automatic TLS)
```

Open TCP 80/443, TCP+UDP 3478, TCP 5349 and the TURN relay port range
in the VPS firewall. Details: `networking.md` → *Deployment (Docker Compose)*.

## Option B: the prebuilt binary with systemd

```bash
sudo install -m 755 mist-signaling-server /usr/local/bin/
sudo mkdir -p /etc/mist
sudo tee /etc/mist/signaling.env >/dev/null <<'ENV'
MIST_SIGNAL_BIND=0.0.0.0:443
MIST_TLS_CERT=/etc/letsencrypt/live/signal.example.org/fullchain.pem
MIST_TLS_KEY=/etc/letsencrypt/live/signal.example.org/privkey.pem
MIST_STUN_URLS=stun:signal.example.org:3478
MIST_TURN_URLS=turn:signal.example.org:3478?transport=udp,turn:signal.example.org:3478?transport=tcp
MIST_TURN_SECRET=<same secret as coturn's static-auth-secret>
ENV
sudo chmod 600 /etc/mist/signaling.env
sudo cp systemd/mist-signaling.service /etc/systemd/system/
sudo systemctl enable --now mist-signaling
```

For TURN, install coturn (`apt install coturn`) and use
`coturn/turnserver.conf` with the same secret. Caddy (`caddy/Caddyfile`)
can terminate TLS instead of the server itself.

## Using it from Mist

On **both** PCs: Settings → Network → Server URL `wss://signal.example.org`.
On the **host**: Settings → Host → turn off *Run Mist's server on this PC*.
Then pair as usual (Host → Create pairing code; Connect → enter the code).
