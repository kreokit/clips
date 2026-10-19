import { useEffect, useState } from "react";
import { clamp, parseNumberField } from "./logic";

/**
 * A compact number input with a unit: type a value (Enter or leaving the field applies
 * it, Escape restores), or use Up / Down (Shift for 10x).
 */
export function NumberField({
  value,
  onChange,
  label,
  min,
  max,
  step = 0.5,
  unit,
}: {
  value: number;
  onChange: (value: number) => void;
  label: string;
  min: number;
  max: number;
  step?: number;
  unit?: string;
}) {
  const shown = value.toFixed(1);
  const [text, setText] = useState(shown);
  useEffect(() => setText(shown), [shown]);

  const commit = () => {
    const n = parseNumberField(text, min, max);
    if (n === null) setText(shown);
    else if (n !== value) onChange(n);
    else setText(shown);
  };

  return (
    <label className="ui-number">
      <span className="ui-number-label">{label}</span>
      <input
        className="ui-input tabular"
        value={text}
        inputMode="decimal"
        aria-label={label}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          else if (e.key === "Escape") setText(shown);
          else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            const by = (e.shiftKey ? step * 10 : step) * (e.key === "ArrowUp" ? 1 : -1);
            onChange(clamp(Math.round((value + by) * 10) / 10, min, max));
          }
        }}
      />
      {unit && <span className="ui-number-unit">{unit}</span>}
    </label>
  );
}
