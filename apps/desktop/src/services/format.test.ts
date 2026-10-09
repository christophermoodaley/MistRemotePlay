import { describe, expect, it } from "vitest";
import { fmtAgo, fmtKbps, fmtMs, endReasonText, sessionStateText } from "./format";
import { applyPreset, validateStream } from "./presets";
import type { StreamSettings } from "../types/api";

const base: StreamSettings = {
  preset: "balanced",
  width: 1920,
  height: 1080,
  fps: 60,
  max_bitrate_kbps: 15000,
  min_bitrate_kbps: 2000,
  adaptive_bitrate: true,
  codec: "h264",
  encoder_quality: "balanced",
  encoder: null,
  audio: { enabled: true, bitrate_kbps: 128, channels: 2 },
};

describe("format", () => {
  it("formats units and missing values", () => {
    expect(fmtMs(null)).toBe("—");
    expect(fmtMs(7.25)).toBe("7.3 ms");
    expect(fmtKbps(12391)).toBe("12.4 Mbps");
    expect(fmtKbps(740)).toBe("740 kbps");
    expect(fmtAgo(null)).toBe("never");
    expect(fmtAgo(1_000, 1_000 + 120_000)).toBe("2 min ago");
  });
  it("describes session states", () => {
    expect(sessionStateText({ state: "connected" })).toBe("Streaming");
    expect(endReasonText({ connection_failed: "ICE" })).toContain("ICE");
    expect(sessionStateText({ state: "ended", detail: "kicked_by_host" })).toContain("disconnected by the host");
  });
});

describe("presets", () => {
  it("matches the Rust presets", () => {
    const low = applyPreset(base, "low_bandwidth");
    expect([low.width, low.height, low.fps, low.max_bitrate_kbps]).toEqual([1280, 720, 30, 4000]);
    expect(low.audio.bitrate_kbps).toBe(64);
    const hq = applyPreset(base, "high_quality");
    expect(hq.max_bitrate_kbps).toBe(40000);
    expect(applyPreset(hq, "custom").max_bitrate_kbps).toBe(40000);
  });
  it("validates like the Rust side", () => {
    expect(validateStream(base)).toBeNull();
    expect(validateStream({ ...base, width: 1921 })).toMatch(/even/);
    expect(validateStream({ ...base, min_bitrate_kbps: 20000 })).toMatch(/Minimum/);
  });
});
