#!/usr/bin/env python3
"""End-to-end test: signaling server + host + client as separate processes.

Steps: start the signaling server, start `mist host`, pair a client with the
code the host prints (approving on the host's stdin), connect the client
headless, approve the session, and check that the client received live,
changing video with capture timestamps. Optionally runs host and client in
separate network namespaces, with direct UDP between them blocked so media
must use a TURN relay.

Usage: tests/e2e/e2e.py [--test-pattern] [--duration N] [--netns] [--relay-only]
                        [--block-direct] [--turn] [--tls] [--input-check]
Requires a built workspace (cargo build) and, for screen capture, an X
display in $DISPLAY.
"""

import argparse
import json
import os
import queue
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.environ.get("MIST_BIN_DIR", os.path.join(ROOT, "target", "debug"))
MIST = os.path.join(BIN, "mist")
SERVER = os.path.join(BIN, "mist-signaling-server")

procs = []


def log(msg):
    print(f"[e2e {time.strftime('%H:%M:%S')}] {msg}", flush=True)


def spawn(cmd, env=None, stdin=False, netns=None, name=""):
    full = (["ip", "netns", "exec", netns] if netns else []) + cmd
    p = subprocess.Popen(
        full,
        stdin=subprocess.PIPE if stdin else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env={**os.environ, **(env or {})},
        text=True,
        bufsize=1,
    )
    p.name = name
    p.lines = queue.Queue()
    p.stderr_tail = []

    def pump_out():
        for line in p.stdout:
            p.lines.put(line.rstrip("\n"))

    def pump_err():
        for line in p.stderr:
            p.stderr_tail.append(line.rstrip("\n"))
            del p.stderr_tail[:-200]

    threading.Thread(target=pump_out, daemon=True).start()
    threading.Thread(target=pump_err, daemon=True).start()
    procs.append(p)
    return p


def wait_event(p, pred, timeout, what):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            line = p.lines.get(timeout=0.2)
        except queue.Empty:
            if p.poll() is not None:
                break
            continue
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if pred(ev):
            return ev
    tails = "\n".join(f"--- {q.name} (exit {q.poll()}) ---\n" + "\n".join(q.stderr_tail[-15:]) for q in procs)
    raise SystemExit(f"timed out waiting for {what} from {p.name}\n{tails}")


def check_input(recorder):
    events = []
    while not recorder.lines.empty():
        try:
            events.append(json.loads(recorder.lines.get_nowait()))
        except (json.JSONDecodeError, queue.Empty):
            pass
    keys = [(e["keycode"], e["down"]) for e in events if e["event"] == "key"]
    buttons = [(e["button"], e["down"]) for e in events if e["event"] == "button"]
    motions = [(e["x"], e["y"]) for e in events if e["event"] == "motion"]
    log(f"recorder saw keys={keys} buttons={buttons} motions={motions[:4]}...")
    # X keycodes: 'A' = 38, Left Shift = 50.
    checks = {
        "key A press+release": (38, True) in keys and (38, False) in keys,
        "left click": (1, True) in buttons and (1, False) in buttons,
        "scroll up": (4, True) in buttons,
        "pointer moved": len(motions) > 0,
        "held Shift released after disconnect": (50, True) in keys and keys.index((50, True)) < len(keys) - 1
        and (50, False) in keys[keys.index((50, True)):],
    }
    for name, passed in checks.items():
        log(f"  input check {'ok  ' if passed else 'FAIL'} {name}")
    return all(checks.values())


def cleanup():
    for p in procs:
        if p.poll() is None:
            p.send_signal(signal.SIGINT)
    time.sleep(1)
    for p in procs:
        if p.poll() is None:
            p.kill()


def sh(cmd):
    subprocess.run(cmd, shell=True, check=True)


def setup_netns(block_direct, block_udp=False):
    """Two namespaces joined through a bridge namespace-less veth pair.

    host ns: 10.77.0.2, client ns: 10.77.0.3, root ns (server + TURN): 10.77.0.1
    With block_direct, UDP/TCP between .2 and .3 is dropped, so only
    relayed media through 10.77.0.1 can work.
    """
    sh("ip netns del mist-h 2>/dev/null || true; ip netns del mist-c 2>/dev/null || true; ip link del mist-br 2>/dev/null || true")
    sh("ip link add mist-br type bridge && ip addr add 10.77.0.1/24 dev mist-br && ip link set mist-br up")
    for ns, ip in (("mist-h", "10.77.0.2"), ("mist-c", "10.77.0.3")):
        sh(f"ip netns add {ns}")
        sh(f"ip link add {ns}-v type veth peer name {ns}-p")
        sh(f"ip link set {ns}-p netns {ns}")
        sh(f"ip link set {ns}-v master mist-br up")
        sh(f"ip netns exec {ns} ip addr add {ip}/24 dev {ns}-p")
        sh(f"ip netns exec {ns} ip link set {ns}-p up")
        sh(f"ip netns exec {ns} ip link set lo up")
        sh(f"ip netns exec {ns} ip route add default via 10.77.0.1")
    if block_direct:
        # Simulate peers that cannot reach each other directly (e.g. both
        # behind symmetric NATs): only the server/TURN address is reachable.
        sh("ip netns exec mist-h iptables -A OUTPUT -d 10.77.0.3 -j DROP")
        sh("ip netns exec mist-c iptables -A OUTPUT -d 10.77.0.2 -j DROP")
    if block_udp:
        for ns in ("mist-h", "mist-c"):
            sh(f"ip netns exec {ns} iptables -A OUTPUT -p udp -j DROP")


def teardown_netns():
    sh("ip netns del mist-h 2>/dev/null || true; ip netns del mist-c 2>/dev/null || true; ip link del mist-br 2>/dev/null || true")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--test-pattern", action="store_true", help="host streams a synthetic pattern instead of $DISPLAY")
    ap.add_argument("--duration", type=int, default=10)
    ap.add_argument("--netns", action="store_true", help="run host and client in separate network namespaces")
    ap.add_argument("--block-direct", action="store_true", help="drop traffic between host and client namespaces")
    ap.add_argument("--block-udp", action="store_true", help="drop all UDP in both namespaces (only TURN over TCP can work)")
    ap.add_argument("--impair-loss", type=float, default=0.0,
                    help="randomly drop this fraction of all packets the host sends (netns only)")
    ap.add_argument("--turn", action="store_true", help="start a local coturn and hand out TURN credentials")
    ap.add_argument("--turn-tcp", action="store_true", help="offer TURN only over TCP")
    ap.add_argument("--relay-only", action="store_true", help="force ICE relay policy on both peers")
    ap.add_argument("--tls", action="store_true", help="serve signaling over TLS with a generated CA")
    ap.add_argument("--audio", default="default", help="host audio: default|off|test-tone")
    ap.add_argument("--width", type=int, default=1280)
    ap.add_argument("--height", type=int, default=720)
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--json-out", help="write the client summary and last stats here")
    ap.add_argument("--input-check", action="store_true",
                    help="inject input with XTest into $DISPLAY and verify it with an X event recorder")
    ap.add_argument("--interrupt", type=float, default=0.0,
                    help="netns only: cut all client traffic for this many seconds mid-session, then restore it")
    ap.add_argument("--window-display", help="run the client with its real window on this X display and screenshot it")
    ap.add_argument("--kill-client", action="store_true",
                    help="SIGKILL the client mid-session; the host must release held input by itself")
    args = ap.parse_args()

    tmp = tempfile.mkdtemp(prefix="mist-e2e-")
    host_dir, client_dir = os.path.join(tmp, "host"), os.path.join(tmp, "client")
    server_ip = "10.77.0.1" if args.netns else "127.0.0.1"
    port = 18443
    env_server = {
        "MIST_SIGNAL_BIND": f"{server_ip}:{port}",
        "MIST_STUN_URLS": "",
        "MIST_LOG": "info",
    }
    if args.netns:
        setup_netns(args.block_direct, args.block_udp)
        if args.impair_loss > 0:
            sh(f"ip netns exec mist-h iptables -A OUTPUT -m statistic --mode random --probability {args.impair_loss} -j DROP")
        if not args.tls:
            env_server["MIST_ALLOW_INSECURE_WS"] = "1"
    ca_args = []
    scheme = "ws"
    if args.tls:
        cert, key = os.path.join(tmp, "cert.pem"), os.path.join(tmp, "key.pem")
        ca_cert, ca_key = os.path.join(tmp, "ca.pem"), os.path.join(tmp, "ca.key")
        csr, ext = os.path.join(tmp, "server.csr"), os.path.join(tmp, "ext.cnf")
        with open(ext, "w") as f:
            f.write(f"basicConstraints=CA:FALSE\nsubjectAltName=IP:{server_ip},DNS:localhost\nextendedKeyUsage=serverAuth\n")
        for cmd in (
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=Mist Test CA",
             "-keyout", ca_key, "-out", ca_cert],
            ["openssl", "req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=mist-signaling", "-keyout", key, "-out", csr],
            ["openssl", "x509", "-req", "-in", csr, "-CA", ca_cert, "-CAkey", ca_key, "-CAcreateserial", "-days", "2",
             "-extfile", ext, "-out", cert],
        ):
            subprocess.run(cmd, check=True, capture_output=True)
        env_server.update({"MIST_TLS_CERT": cert, "MIST_TLS_KEY": key})
        ca_args = ["--ca", ca_cert]
        scheme = "wss"
    turn_secret = None
    if args.turn:
        turn_secret = os.urandom(16).hex()
        transports = ["tcp"] if args.turn_tcp else ["udp", "tcp"]
        env_server["MIST_TURN_URLS"] = ",".join(f"turn:{server_ip}:3478?transport={t}" for t in transports)
        env_server["MIST_TURN_SECRET"] = turn_secret
        conf = os.path.join(tmp, "turnserver.conf")
        with open(conf, "w") as f:
            f.write("\n".join([
                f"listening-ip={server_ip}", f"relay-ip={server_ip}", "listening-port=3478",
                "fingerprint", "use-auth-secret", f"static-auth-secret={turn_secret}", "realm=mist.test",
                "no-tls", "no-dtls", "no-cli", "no-multicast-peers", "min-port=49160", "max-port=49300",
                # The test network is private; production configs must keep
                # private peers denied (see deploy/coturn).
                "allow-loopback-peers" if not args.netns else "", f"log-file={tmp}/turn.log", "simple-log",
            ]) + "\n")
        turn = spawn(["turnserver", "-c", conf], name="coturn")
        time.sleep(1)
        if turn.poll() is not None:
            raise SystemExit("coturn failed to start: " + "\n".join(turn.stderr_tail))

    sig_url = f"{scheme}://{server_ip}:{port}"
    server = spawn([SERVER], env=env_server, name="server")
    time.sleep(1)
    if server.poll() is not None:
        raise SystemExit("signaling server failed:\n" + "\n".join(server.stderr_tail))
    log(f"signaling server at {sig_url}")

    common = ["--json", "--signaling", sig_url] + ca_args + (["--relay-only"] if args.relay_only else [])
    recorder = None
    if args.input_check:
        recorder = spawn([os.path.join(BIN, "examples", "x11_event_recorder")], name="recorder")
        wait_event(recorder, lambda e: e.get("event") == "ready", 10, "recorder ready")
    host_cmd = [MIST, "--config-dir", host_dir] + common + [
        "host", "--pair", "--input", "xtest" if args.input_check else "mock", "--controllers", "mock", "--audio", args.audio,
    ] + (["--test-pattern"] if args.test_pattern else [])
    host = spawn(host_cmd, stdin=True, netns="mist-h" if args.netns else None, name="host",
                 env={"MIST_LOG": "info"})
    ev = wait_event(host, lambda e: e.get("event") == "pairing_code", 20, "pairing code")
    code = ev["code"]
    log(f"host shows pairing code {code}")

    pair = spawn([MIST, "--config-dir", client_dir] + common + ["pair", code],
                 netns="mist-c" if args.netns else None, name="pair")
    wait_event(host, lambda e: e.get("event") == "pairing_request", 20, "pairing request")
    log("host received pairing request; approving")
    host.stdin.write("y\n")
    host.stdin.flush()
    wait_event(pair, lambda e: e.get("event") == "paired", 20, "pairing result")
    log("client paired")

    if args.window_display:
        client = spawn(
            [MIST, "--config-dir", client_dir] + common + [
                "connect", "--duration", str(args.duration), "--width", str(args.width), "--height", str(args.height),
                "--fps", str(args.fps), "--no-controllers"],
            env={"DISPLAY": args.window_display}, name="client-window")
        wait_event(host, lambda e: e.get("event") == "session_request", 30, "session request")
        host.stdin.write("y\n")
        host.stdin.flush()
        wait_event(client, lambda e: e.get("event") == "state" and e.get("state") == "streaming", 40, "streaming state")
        time.sleep(4)
        shot = os.path.join(ROOT, "docs", "images", "client-window.png")
        os.makedirs(os.path.dirname(shot), exist_ok=True)
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-i", args.window_display,
                        "-frames:v", "1", shot], check=True)
        stats = None
        deadline = time.time() + args.duration
        while time.time() < deadline:
            try:
                ev = json.loads(client.lines.get(timeout=0.5))
                if ev.get("event") == "stats":
                    stats = ev
            except (queue.Empty, json.JSONDecodeError):
                pass
        log(f"screenshot saved to {shot}")
        if stats:
            s = stats["stats"]
            log(f"window client: rendered_fps={s['rendered_fps']:.1f} latency={s.get('latency_ms')} dropped={s['frames_dropped']}")
        ok = stats is not None and stats["stats"]["rendered_fps"] > args.fps * 0.5
        log("PASS" if ok else "FAIL")
        return 0 if ok else 1

    client = spawn(
        [MIST, "--config-dir", client_dir] + common + [
            "connect", "--headless", "--duration", str(args.duration),
            "--width", str(args.width), "--height", str(args.height), "--fps", str(args.fps), "--no-controllers",
        ] + (["--input-selftest"] if args.input_check else []),
        netns="mist-c" if args.netns else None, name="client")
    wait_event(host, lambda e: e.get("event") == "session_request", 30, "session request")
    log("host received session request; approving")
    host.stdin.write("y\n")
    host.stdin.flush()
    wait_event(client, lambda e: e.get("event") == "state" and e.get("state") == "streaming", 40, "streaming state")
    log("client streaming")

    if args.kill_client:
        time.sleep(4)
        log("killing client with SIGKILL (no goodbye, no release)")
        client.kill()
        t0 = time.time()
        ev = wait_event(host, lambda e: e.get("event") == "session" and isinstance(e.get("state"), dict)
                        and e["state"].get("state") == "ended", 60, "host-side session end")
        log(f"host ended the session {time.time() - t0:.1f}s after the client vanished: {ev['state']}")
        ok = True
        if recorder:
            time.sleep(1)
            ok = check_input(recorder)
        log("PASS" if ok else "FAIL")
        return 0 if ok else 1

    states_after_cut = []
    if args.interrupt > 0:
        time.sleep(4)
        log(f"cutting all client network traffic for {args.interrupt:.0f}s")
        sh("ip netns exec mist-c iptables -I INPUT 1 -j DROP; ip netns exec mist-c iptables -I OUTPUT 1 -j DROP")
        time.sleep(args.interrupt)
        sh("ip netns exec mist-c iptables -D INPUT 1; ip netns exec mist-c iptables -D OUTPUT 1")
        log("network restored")
        t0 = time.time()
        ev = wait_event(client, lambda e: (e.get("event") == "state" and states_after_cut.append(e["state"]) is None
                                           and e.get("state") == "streaming"), 60, "recovery to streaming")
        log(f"client back to streaming {time.time() - t0:.1f}s after restore (states seen: {states_after_cut})")

    last_stats, summary, host_stats = None, None, None
    deadline = time.time() + args.duration + 30
    while time.time() < deadline and summary is None:
        try:
            line = client.lines.get(timeout=0.5)
        except queue.Empty:
            if client.poll() is not None:
                break
            continue
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("event") == "stats":
            last_stats = ev
        elif ev.get("event") == "summary":
            summary = ev
    while not host.lines.empty():
        try:
            ev = json.loads(host.lines.get_nowait())
            if ev.get("event") == "stats":
                host_stats = ev
        except (json.JSONDecodeError, queue.Empty):
            pass

    if summary is None:
        raise SystemExit("client produced no summary\n" + "\n".join(client.stderr_tail[-40:]))
    log(f"summary: {json.dumps(summary)}")
    if last_stats:
        s = last_stats["stats"]
        log(
            "last client stats: fps={:.1} latency={} rtt={} decode={:.2}ms lost={} dropped={} relayed={} kbps={}".format(
                s["rendered_fps"], s.get("latency_ms"), last_stats.get("rtt_ms"), s["decode_ms_avg"],
                s["packets_lost"], s["frames_dropped"], last_stats.get("relayed"), last_stats.get("video_bitrate_kbps")))
        t = last_stats.get("transport", {})
        log(f"selected candidates: local={t.get('local_candidate_type')}/{t.get('local_candidate_protocol')} remote={t.get('remote_candidate_type')} relay_protocol={t.get('relay_protocol')}")
        aud = (t.get("inbound_audio") or {})
        log(f"audio packets received: {aud.get('packets')}")
    if host_stats:
        h = host_stats["host"]
        tr = host_stats.get("transport", {})
        log(f"host transport: rtt={tr.get('remote_rtt_ms')} fraction_lost={tr.get('remote_fraction_lost')} nacks={((tr.get('outbound_video') or {}).get('nack_count'))} plis={((tr.get('outbound_video') or {}).get('pli_count'))}")
        log(f"host: encoder={h['encoder']} {h['width']}x{h['height']} capture_fps={h['capture_fps']:.1f} encode_ms={h['encode_ms_avg']:.2f} target_kbps={h['target_bitrate_kbps']}")

    # Expect at least half the frames of the time actually streamed.
    streamed_secs = max(1.0, args.duration - (args.interrupt + 15 if args.interrupt else 0))
    ok = (
        summary["frames"] >= args.fps * streamed_secs / 2
        and summary["distinct_frames"] >= 3
        and summary["frames_with_capture_timestamp"] > 0
    )
    if args.relay_only or args.block_direct or args.block_udp:
        ok = ok and last_stats is not None and last_stats.get("relayed") is True
    if args.audio != "off":
        ok = ok and last_stats is not None and ((last_stats.get("transport") or {}).get("inbound_audio") or {}).get("packets", 0) > 0
    if recorder:
        time.sleep(2)
        ok = check_input(recorder) and ok
    if args.json_out:
        with open(args.json_out, "w") as f:
            json.dump({"summary": summary, "client_stats": last_stats, "host_stats": host_stats, "args": vars(args)}, f, indent=2)
    log("PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    rc = 1
    try:
        rc = main()
    finally:
        cleanup()
        if "--netns" in sys.argv:
            teardown_netns()
    sys.exit(rc)
