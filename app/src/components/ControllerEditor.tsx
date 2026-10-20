import { useState } from "react";
import { api, errorMessage } from "../services/api";
import type { ConnectedController, ControllerMapping } from "../types/api";
import { Icon } from "./Icon";
import { Badge, Button, Card, Field, Select, Slider, Toggle } from "./ui";

// Standard layout button names (core/src/protocol/input.rs `buttons::NAMES`).
const BUTTONS = [
  "a",
  "b",
  "x",
  "y",
  "left_shoulder",
  "right_shoulder",
  "back",
  "start",
  "guide",
  "left_stick",
  "right_stick",
  "dpad_up",
  "dpad_down",
  "dpad_left",
  "dpad_right",
] as const;

const LABEL: Record<string, string> = {
  a: "A / Cross",
  b: "B / Circle",
  x: "X / Square",
  y: "Y / Triangle",
  left_shoulder: "Left bumper",
  right_shoulder: "Right bumper",
  back: "Back / Select",
  start: "Start",
  guide: "Guide / Home",
  left_stick: "Left stick press",
  right_stick: "Right stick press",
  dpad_up: "D-pad up",
  dpad_down: "D-pad down",
  dpad_left: "D-pad left",
  dpad_right: "D-pad right",
};

export function ControllerEditor({
  enabled,
  mapping,
  onEnabled,
  onChange,
}: {
  enabled: boolean;
  mapping: ControllerMapping;
  onEnabled: (v: boolean) => void;
  onChange: (m: ControllerMapping) => void;
}) {
  const [detected, setDetected] = useState<ConnectedController[] | null>(null);
  const [detectError, setDetectError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);

  async function detect() {
    setScanning(true);
    try {
      setDetected(await api.detectControllers());
      setDetectError(null);
    } catch (e) {
      setDetectError(errorMessage(e));
    } finally {
      setScanning(false);
    }
  }

  const remapped = Object.keys(mapping.remap).length;

  return (
    <div className="space-y-6">
      <Card title="Controllers" icon="gamepad" actions={<Button size="sm" variant="ghost" icon="refresh" busy={scanning} onClick={detect}>Detect</Button>}>
        <Toggle label="Forward controllers to the host" description="Up to four controllers, mapped to a standard layout." checked={enabled} onChange={onEnabled} />
        <div className="mt-3">
          {detectError && <p className="text-sm text-bad">{detectError}</p>}
          {detected === null ? (
            <p className="text-sm text-muted">Press Detect to list controllers connected to this computer.</p>
          ) : detected.length === 0 ? (
            <p className="text-sm text-muted">No controllers found.</p>
          ) : (
            <ul className="space-y-2">
              {detected.map((c) => (
                <li key={c.slot} className="flex items-center gap-3 text-sm">
                  <Icon name="gamepad" className="text-accent" />
                  <span className="flex-1">{c.name}</span>
                  <Badge>{c.kind.replace("_", " ")}</Badge>
                  <Badge tone="accent">Player {c.slot + 1}</Badge>
                </li>
              ))}
            </ul>
          )}
        </div>
      </Card>

      <Card title="Sticks and triggers" icon="sliders">
        <div className="grid grid-cols-3 gap-6">
          <Field label="Left stick deadzone">{(id) => <Slider id={id} min={0} max={0.5} step={0.01} value={mapping.left_deadzone} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => onChange({ ...mapping, left_deadzone: v })} />}</Field>
          <Field label="Right stick deadzone">{(id) => <Slider id={id} min={0} max={0.5} step={0.01} value={mapping.right_deadzone} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => onChange({ ...mapping, right_deadzone: v })} />}</Field>
          <Field label="Trigger deadzone">{(id) => <Slider id={id} min={0} max={0.5} step={0.01} value={mapping.trigger_deadzone} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => onChange({ ...mapping, trigger_deadzone: v })} />}</Field>
        </div>
        <div className="mt-3 grid grid-cols-2 gap-x-8">
          <Toggle label="Invert left stick Y" checked={mapping.invert_left_y} onChange={(v) => onChange({ ...mapping, invert_left_y: v })} />
          <Toggle label="Invert right stick Y" checked={mapping.invert_right_y} onChange={(v) => onChange({ ...mapping, invert_right_y: v })} />
        </div>
      </Card>

      <Card
        title="Button mapping"
        icon="keyboard"
        actions={
          remapped > 0 && (
            <Button size="sm" variant="ghost" onClick={() => onChange({ ...mapping, remap: {} })}>
              Reset ({remapped})
            </Button>
          )
        }
      >
        <p className="mb-4 text-sm text-muted">Each physical button can send a different standard button to the host.</p>
        <div className="grid grid-cols-3 gap-x-6 gap-y-3">
          {BUTTONS.map((b) => (
            <Field key={b} label={LABEL[b]}>
              {(id) => (
                <Select
                  id={id}
                  value={mapping.remap[b] ?? b}
                  onChange={(v) => {
                    const remap = { ...mapping.remap };
                    if (v === b) delete remap[b];
                    else remap[b] = v;
                    onChange({ ...mapping, remap });
                  }}
                  options={BUTTONS.map((t) => ({ value: t, label: t === b ? `${LABEL[t]} (default)` : LABEL[t] }))}
                />
              )}
            </Field>
          ))}
        </div>
      </Card>
    </div>
  );
}
