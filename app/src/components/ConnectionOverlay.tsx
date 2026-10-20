import { useEffect, useState } from "react";
import { api, onOverlayViewers, type OverlayViewer } from "../services/api";
import { Icon } from "./Icon";
import { LiveDot } from "./ui";

function names(viewers: OverlayViewer[]): string {
  const n = viewers.map((v) => v.name || "Someone");
  if (n.length <= 2) return n.join(" and ");
  if (n.length === 3) return `${n[0]}, ${n[1]} and ${n[2]}`;
  return `${n[0]}, ${n[1]} and ${n.length - 2} others`;
}

function roles(viewers: OverlayViewer[]): string {
  const playing = viewers.filter((v) => v.role === "play").length;
  const watching = viewers.length - playing;
  if (viewers.length === 1) return playing ? "Playing on this PC" : "Watching this screen";
  return [playing && `${playing} playing`, watching && `${watching} watching`].filter(Boolean).join(" · ");
}

/** The host-only "someone is connected" notice (window label "overlay"). It
 *  is click-through and left out of the stream by the Rust side. */
export function ConnectionOverlay() {
  const [viewers, setViewers] = useState<OverlayViewer[]>([]);

  useEffect(() => {
    document.documentElement.classList.add("overlay");
    api.overlayViewers().then(setViewers).catch(() => {});
    const un = onOverlayViewers(setViewers);
    return () => {
      un.then((f) => f());
    };
  }, []);

  if (viewers.length === 0) return null;
  const many = viewers.length > 1;

  return (
    <div className="flex h-full items-start justify-center p-1">
      <div className="fade-in flex max-w-full items-center gap-3 rounded-full border border-white/10 bg-[#0a0e13]/90 py-2 pl-2.5 pr-5 shadow-lg shadow-black/40">
        <span className="relative flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-accent/15 text-accent">
          <Icon name={many ? "users" : viewers[0].role === "play" ? "gamepad" : "eye"} size={18} />
          <span className="absolute -right-0.5 -top-0.5">
            <LiveDot tone="bad" live size={9} />
          </span>
        </span>
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold text-ink">
            {names(viewers)} {many ? "are" : "is"} connected
          </div>
          <div className="truncate text-xs text-muted">{roles(viewers)} · Only you can see this</div>
        </div>
      </div>
    </div>
  );
}
