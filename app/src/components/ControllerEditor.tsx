import type { ControllerMapping } from "../types/api";
import { Button, Field, Select, Slider, Toggle } from "./ui";

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

/** Stick, trigger and button tuning for forwarded controllers. */
export function ControllerEditor({ mapping, onChange }: { mapping: ControllerMapping; onChange: (m: ControllerMapping) => void }) {
  const remapped = Object.keys(mapping.remap).length;

  return (
    <div className="space-y-6">
      <div>
        <h3 className="mb-3 text-sm font-semibold">Sticks and triggers</h3>
        <div className="grid grid-cols-3 gap-6">
          <Field label="Left stick deadzone">{(id) => <Slider id={id} min={0} max={0.5} step={0.01} value={mapping.left_deadzone} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => onChange({ ...mapping, left_deadzone: v })} />}</Field>
          <Field label="Right stick deadzone">{(id) => <Slider id={id} min={0} max={0.5} step={0.01} value={mapping.right_deadzone} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => onChange({ ...mapping, right_deadzone: v })} />}</Field>
          <Field label="Trigger deadzone">{(id) => <Slider id={id} min={0} max={0.5} step={0.01} value={mapping.trigger_deadzone} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => onChange({ ...mapping, trigger_deadzone: v })} />}</Field>
        </div>
        <div className="mt-3 grid grid-cols-2 gap-x-8">
          <Toggle label="Invert left stick Y" checked={mapping.invert_left_y} onChange={(v) => onChange({ ...mapping, invert_left_y: v })} />
          <Toggle label="Invert right stick Y" checked={mapping.invert_right_y} onChange={(v) => onChange({ ...mapping, invert_right_y: v })} />
        </div>
      </div>

      <div className="border-t border-line pt-5">
        <div className="mb-1 flex items-center justify-between gap-3">
          <h3 className="text-sm font-semibold">Buttons</h3>
          {remapped > 0 && (
            <Button size="sm" variant="ghost" onClick={() => onChange({ ...mapping, remap: {} })}>
              Reset ({remapped})
            </Button>
          )}
        </div>
        <p className="mb-4 text-sm text-muted">Each button can send a different one to the host.</p>
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
      </div>
    </div>
  );
}
