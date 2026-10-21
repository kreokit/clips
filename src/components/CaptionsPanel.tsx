import { Check } from "lucide-react";
import { outlineShadow } from "../lib/captions/overlay";
import { anchorForY, anchorPoint } from "../lib/captions/position";
import { FONTS, PRESETS, Preset, matchingPreset } from "../lib/captions/presets";
import type { CaptionMode, Position, Style } from "../lib/captions/types";
import { ColorPicker, Field, NumberField, SectionLabel, Segmented, Select, Slider, Switch } from "../ui";

const MODES: { value: CaptionMode; label: string }[] = [
  { value: "line", label: "Line" },
  { value: "word_highlight", label: "Highlight" },
  { value: "one_word", label: "One word" },
];

const POSITIONS: { value: Position; label: string }[] = [
  { value: "top", label: "Top" },
  { value: "middle", label: "Middle" },
  { value: "bottom", label: "Bottom" },
];

const LINES = [
  { value: "1", label: "1" },
  { value: "2", label: "2" },
  { value: "3", label: "3" },
];

/**
 * The project's caption look (one style for every clip, decided 2026-10-21): presets as
 * rendered tiles, then Text / Colors / Layout. From KreoKit Captions' style panel.
 * `fitted` is the style as this clip shows it (shrunk if its longest caption is too wide).
 */
export function CaptionsPanel({
  style,
  fitted,
  onStyle,
  showSafeArea,
  onShowSafeArea,
}: {
  style: Style;
  fitted: Style | undefined;
  onStyle: (patch: Partial<Style>) => void;
  showSafeArea: boolean;
  onShowSafeArea: (show: boolean) => void;
}) {
  const setStyle = onStyle;
  const active = matchingPreset(style);
  const set = <K extends keyof Style>(key: K, value: Style[K]) => setStyle({ [key]: value });

  return (
    <div className="style-panel">
      <section className="style-section">
        <SectionLabel>Presets</SectionLabel>
        <div className="preset-grid" role="radiogroup" aria-label="Caption presets">
          {PRESETS.map((p) => (
            <PresetTile key={p.id} preset={p} selected={p.id === active?.id} onSelect={() => setStyle(p.style)} />
          ))}
        </div>
      </section>

      <section className="style-section">
        <SectionLabel>Text</SectionLabel>
        <Field label="Font" inline>
          <Select
            label="Font"
            value={style.font_family}
            onChange={(v) => set("font_family", v)}
            options={FONTS.map((f) => ({ value: f, label: f, fontFamily: `"${f}"` }))}
          />
        </Field>
        <SliderField label="Size" value={style.font_size} min={24} max={160} onChange={(v) => set("font_size", v)} />
        {fitted && fitted.font_size < style.font_size && (
          <p className="style-hint">
            Shrunk to {fitted.font_size} in this clip so its longest caption fits the frame. Fewer characters per
            line keeps it bigger.
          </p>
        )}
        <Switch label="Bold" checked={style.bold} onChange={(v) => set("bold", v)} />
        <Switch label="Uppercase" checked={style.uppercase} onChange={(v) => set("uppercase", v)} />
        <Switch label="Pop in" checked={style.animation === "pop"} onChange={(v) => set("animation", v ? "pop" : "none")} />
      </section>

      <section className="style-section">
        <SectionLabel>Colors</SectionLabel>
        <Field label="Text" inline>
          <ColorPicker label="Text color" value={style.color} onChange={(v) => set("color", v)} />
        </Field>
        <Field label="Highlight" inline>
          <ColorPicker label="Highlight color" value={style.highlight_color} onChange={(v) => set("highlight_color", v)} />
        </Field>
        <Field label="Outline" inline>
          <ColorPicker label="Outline color" value={style.outline_color} onChange={(v) => set("outline_color", v)} />
        </Field>
        <SliderField label="Outline width" value={style.outline_width} min={0} max={12} onChange={(v) => set("outline_width", v)} />
      </section>

      <section className="style-section">
        <SectionLabel>Layout</SectionLabel>
        <Field label="Show">
          <Segmented label="Show" value={style.mode} options={MODES} onChange={(v) => set("mode", v)} />
        </Field>
        <PositionFields style={style} onStyle={onStyle} showSafeArea={showSafeArea} onShowSafeArea={onShowSafeArea} />
        <SliderField
          label="Characters per line"
          value={style.max_chars_per_line}
          min={8}
          max={60}
          onChange={(v) => set("max_chars_per_line", v)}
        />
        <Field label="Lines" inline>
          <Segmented
            label="Lines"
            value={String(style.max_lines)}
            options={LINES}
            onChange={(v) => set("max_lines", Number(v))}
          />
        </Field>
      </section>
    </div>
  );
}

const ANCHOR_HINT: Record<Position, string> = {
  top: "X is the caption's centre, Y its top edge.",
  middle: "X and Y are the caption's centre.",
  bottom: "X is the caption's centre, Y its bottom edge.",
};

/**
 * Top / Middle / Bottom spots (selected only while no free position is set), exact X / Y
 * in percent of the frame, and the social safe-area overlay.
 */
function PositionFields({
  style,
  onStyle,
  showSafeArea,
  onShowSafeArea,
}: {
  style: Style;
  onStyle: (patch: Partial<Style>) => void;
  showSafeArea: boolean;
  onShowSafeArea: (show: boolean) => void;
}) {
  const setStyle = onStyle;
  const free = style.pos_x !== null || style.pos_y !== null;
  const { x, y } = anchorPoint(style);
  return (
    <>
      <Field label="Position" hint={`Drag the caption on the preview to place it freely. ${ANCHOR_HINT[style.position]}`}>
        <Segmented
          label="Position"
          value={free ? ("" as Position) : style.position}
          options={POSITIONS}
          onChange={(v) => setStyle({ position: v, pos_x: null, pos_y: null })}
        />
        <div className="position-xy">
          <NumberField
            label="X"
            unit="%"
            min={0}
            max={100}
            value={x * 100}
            onChange={(v) => setStyle({ pos_x: v / 100, pos_y: y })}
          />
          <NumberField
            label="Y"
            unit="%"
            min={0}
            max={100}
            value={y * 100}
            onChange={(v) => setStyle({ pos_x: x, pos_y: v / 100, position: anchorForY(v / 100) })}
          />
        </div>
      </Field>
      <Switch label="Show social safe area" checked={showSafeArea} onChange={onShowSafeArea} />
    </>
  );
}

function SliderField(props: { label: string; value: number; min: number; max: number; onChange: (v: number) => void }) {
  return (
    <div className="slider-field">
      <div className="slider-field-head">
        <span className="ui-field-label">{props.label}</span>
        <span className="slider-field-value tabular">{props.value}</span>
      </div>
      <Slider label={props.label} value={props.value} min={props.min} max={props.max} onChange={props.onChange} />
    </div>
  );
}

/** Sample shown in each preset tile: "make it pop" with "pop" being spoken. */
const SAMPLE = ["Make", "it", "pop"];
const SPOKEN = 2;
/** Tile previews draw the style at this fraction of its 1080 px reference size. */
const TILE_SCALE = 0.26;

/** Width of a tile's sample area in the 312 px side panel (3 tiles), in px. */
const SAMPLE_WIDTH = 80;

/** The style's size scaled for the tile, shrunk so `chars` characters fit the sample
 * area (about 0.6 em per character, 0.75 em for bold capitals). */
function tileFontSize(style: Style, chars: number): number {
  const scaled = Math.min(style.font_size * TILE_SCALE, style.mode === "one_word" ? 40 : 30);
  const perChar = style.uppercase && style.bold ? 0.75 : 0.6;
  return Math.min(scaled, SAMPLE_WIDTH / (Math.max(chars, 1) * perChar));
}

function PresetTile({ preset, selected, onSelect }: { preset: Preset; selected: boolean; onSelect: () => void }) {
  const s = preset.style;
  const words = (s.mode === "one_word" ? [SAMPLE[SPOKEN]] : SAMPLE).map((w) => (s.uppercase ? w.toUpperCase() : w));
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      className={selected ? "preset-tile selected" : "preset-tile"}
      onClick={onSelect}
    >
      <span className="preset-sample" aria-hidden>
        <span
          className="preset-sample-text"
          style={{
            fontFamily: `"${s.font_family}"`,
            fontWeight: s.bold ? 700 : 400,
            // Scaled from the style, but never wider than the tile's sample area.
            fontSize: tileFontSize(s, words.join(" ").length),
            color: s.color,
            textShadow: outlineShadow(Math.max(s.outline_width * TILE_SCALE, s.outline_width ? 1 : 0), s.outline_color),
          }}
        >
          {words.map((w, i) => {
            const spoken = s.mode === "one_word" || (s.mode === "word_highlight" && i === SPOKEN);
            return (
              <span key={i} style={spoken ? { color: s.highlight_color } : undefined}>
                {w}
                {i < words.length - 1 ? " " : ""}
              </span>
            );
          })}
        </span>
      </span>
      <span className="preset-name">
        {preset.name}
        {selected && (
          <span className="preset-check">
            <Check size={10} strokeWidth={3} aria-hidden />
          </span>
        )}
      </span>
    </button>
  );
}
