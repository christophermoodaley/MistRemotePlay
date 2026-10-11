// Mirrors StreamSettings::preset in crates/protocol/src/settings.rs.
import type { QualityPreset, StreamSettings } from "../types/api";

export const PRESET_LABELS: Record<QualityPreset, string> = {
  low_bandwidth: "Low bandwidth",
  low_latency: "Low latency",
  balanced: "Balanced",
  high_quality: "High quality",
  custom: "Custom",
};

export function applyPreset(current: StreamSettings, preset: QualityPreset): StreamSettings {
  if (preset === "custom") return { ...current, preset };
  const table: Record<Exclude<QualityPreset, "custom">, [number, number, number, number, number]> = {
    low_bandwidth: [1280, 720, 30, 4000, 1000],
    low_latency: [1920, 1080, 60, 20000, 3000],
    balanced: [1920, 1080, 60, 15000, 2000],
    high_quality: [1920, 1080, 60, 40000, 5000],
  };
  const [width, height, fps, max, min] = table[preset];
  return {
    ...current,
    preset,
    width,
    height,
    fps,
    max_bitrate_kbps: max,
    min_bitrate_kbps: min,
    encoder_quality: preset === "low_bandwidth" || preset === "low_latency" ? "fastest" : preset === "high_quality" ? "quality" : "balanced",
    audio: { ...current.audio, bitrate_kbps: preset === "low_bandwidth" ? 64 : 128 },
  };
}

/** Same rules as the Rust `Validate` impl; returns a message or null. */
export function validateStream(s: StreamSettings): string | null {
  if (s.width < 320 || s.width > 7680 || s.width % 2) return "Width must be an even number between 320 and 7680.";
  if (s.height < 240 || s.height > 4320 || s.height % 2) return "Height must be an even number between 240 and 4320.";
  if (s.fps < 10 || s.fps > 240) return "Frame rate must be between 10 and 240.";
  if (s.max_bitrate_kbps < 250 || s.max_bitrate_kbps > 500000) return "Maximum bitrate must be between 250 and 500000 kbps.";
  if (s.min_bitrate_kbps < 100 || s.min_bitrate_kbps > s.max_bitrate_kbps)
    return "Minimum bitrate must be at least 100 kbps and not above the maximum.";
  if (s.audio.bitrate_kbps < 16 || s.audio.bitrate_kbps > 510) return "Audio bitrate must be between 16 and 510 kbps.";
  return null;
}

export const RESOLUTIONS: [number, number][] = [
  [1280, 720],
  [1600, 900],
  [1920, 1080],
  [2560, 1440],
  [3840, 2160],
];
export const FRAME_RATES = [30, 60, 90, 120, 144];
