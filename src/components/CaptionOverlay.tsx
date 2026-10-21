import { useRef, useState } from "react";
import { GripHorizontal } from "lucide-react";
import { POP_MS, POP_SCALE, ghostFrame, outlineShadow, overlayFrame } from "../lib/captions/overlay";
import { Anchor, Guides, REFERENCE_HEIGHT, anchorPoint, dragAnchor, placement, playResX } from "../lib/captions/position";
import { SAFE_AREA } from "../lib/captions/safeArea";
import { emPerFontSize } from "../lib/captions/textMetrics";
import type { Segment, Style } from "../lib/captions/types";

/** The exported frame; captions are placed on it as in the ASS script (608 × 1080). */
export const OUTPUT = { width: 1080, height: 1920 };
const SCRIPT_W = playResX(OUTPUT.width, OUTPUT.height);

/**
 * Shades what TikTok / Reels / Shorts cover with their own interface, so captions can
 * avoid it. Editor only: never exported.
 */
export function SafeAreaOverlay() {
  const pct = (v: number) => `${v * 100}%`;
  return (
    <div className="safe-area" aria-hidden>
      <div className="safe-area-zone" style={{ top: 0, left: 0, right: 0, height: pct(SAFE_AREA.top) }} />
      <div className="safe-area-zone" style={{ bottom: 0, left: 0, right: 0, height: pct(SAFE_AREA.bottom) }}>
        <span>Covered by app captions and buttons</span>
      </div>
      <div
        className="safe-area-zone"
        style={{ top: pct(SAFE_AREA.top), bottom: pct(SAFE_AREA.bottom), right: 0, width: pct(SAFE_AREA.right) }}
      />
    </div>
  );
}

/** Snap distance while dragging a caption, in screen pixels. */
const SNAP_PX = 8;

interface DragStart {
  pointerX: number;
  pointerY: number;
  anchor: Anchor;
  box: { w: number; h: number };
}

/**
 * The clip's caption at `clipMs` on the 9:16 preview (`frame` is its size in CSS
 * pixels), placed like the export (from KreoKit Captions). `style` is the fitted style.
 * It can be dragged anywhere (snapping to the centres and the Top / Bottom spots with
 * lime guides; Alt drags freely) and double-clicked to go back to the Bottom spot. With
 * `ghost`, when nothing is on screen the nearest caption shows dimmed so it can still be
 * placed.
 */
export function CaptionOverlay({
  segments,
  style,
  clipMs,
  frame,
  ghost: showGhost,
  onPlace,
}: {
  segments: Segment[];
  style: Style;
  clipMs: number;
  frame: { width: number; height: number };
  ghost: boolean;
  /** A new position, dropped. */
  onPlace: (patch: Pick<Style, "position" | "pos_x" | "pos_y">) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  const start = useRef<DragStart | null>(null);
  const [drag, setDrag] = useState<(Anchor & { guides: Guides }) | null>(null);

  // While dragging, the caption follows the draft position; the project changes on drop.
  const live = drag ? { ...style, position: drag.position, pos_x: drag.x, pos_y: drag.y } : style;
  let shown = overlayFrame(segments, clipMs, live);
  const ghost = !shown && (showGhost || drag !== null);
  if (ghost) shown = ghostFrame(segments, clipMs, live);
  if (!shown) return null;
  const scale = frame.height / REFERENCE_HEIGHT;

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    const rect = box.current!.getBoundingClientRect();
    const { x, y } = anchorPoint(live);
    start.current = {
      pointerX: e.clientX,
      pointerY: e.clientY,
      anchor: { x, y, position: live.position },
      box: { w: rect.width / frame.width, h: rect.height / frame.height },
    };
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const s = start.current;
    if (!s) return;
    if (!drag && Math.hypot(e.clientX - s.pointerX, e.clientY - s.pointerY) < 3) return;
    const dx = (e.clientX - s.pointerX) / frame.width;
    const dy = (e.clientY - s.pointerY) / frame.height;
    const snap = e.altKey ? null : { x: SNAP_PX / frame.width, y: SNAP_PX / frame.height };
    setDrag(dragAnchor(s.anchor, dx, dy, s.box, snap));
  };
  const onPointerUp = () => {
    if (drag) onPlace({ position: drag.position, pos_x: drag.x, pos_y: drag.y });
    start.current = null;
    setDrag(null);
  };

  // The same anchor, in script pixels, as the ASS export (placement in position.ts).
  const place = placement(live, SCRIPT_W);
  const classes = [
    "overlay",
    `overlay-${live.position}`,
    live.animation === "pop" && !ghost && !drag && "pop",
    "draggable",
    ghost && "ghost",
    drag && "dragging",
  ];
  return (
    <>
      {drag?.guides.vertical !== undefined && (
        <div className="caption-guide caption-guide-v" style={{ left: drag.guides.vertical * frame.width }} />
      )}
      {drag?.guides.horizontal !== undefined && (
        <div className="caption-guide caption-guide-h" style={{ top: drag.guides.horizontal * frame.height }} />
      )}
      <div
        ref={box}
        // A new key per caption restarts the pop animation, like a new ASS event.
        key={live.animation === "pop" ? shown.key : undefined}
        className={classes.filter(Boolean).join(" ")}
        style={{ ...overlayStyle(live, scale), left: place.x * scale, top: place.y * scale }}
        title="Drag to move · double-click for the Bottom spot · hold ⌥ to move freely"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onClick={(e) => e.stopPropagation()}
        onDoubleClick={(e) => {
          e.stopPropagation();
          onPlace({ position: "bottom", pos_x: null, pos_y: null });
        }}
      >
        <span className="overlay-grip" aria-hidden>
          <GripHorizontal size={14} strokeWidth={2} />
        </span>
        {shown.lines.map((line, l) => (
          <div key={l} className="overlay-line">
            {line.map((w, i) => (
              <span key={i} style={w.active ? { color: live.highlight_color } : undefined}>
                {w.text}
                {i < line.length - 1 ? " " : ""}
              </span>
            ))}
          </div>
        ))}
      </div>
    </>
  );
}

/** Font, size, colour and outline from the same reference values as the export. */
function overlayStyle(style: Style, scale: number): React.CSSProperties {
  const size = style.font_size * scale;
  return {
    fontFamily: `"${style.font_family}"`,
    fontWeight: style.bold ? 700 : 400,
    fontSize: `${size * emPerFontSize(style.font_family, style.bold)}px`,
    // One line is exactly the ASS font size tall, as in libass.
    lineHeight: `${size}px`,
    color: style.color,
    // Outline outside the fill with round corners, like libass (see outlineShadow).
    textShadow: outlineShadow(style.outline_width * scale, style.outline_color),
    ["--pop-from" as string]: POP_SCALE,
    ["--pop-ms" as string]: `${POP_MS}ms`,
  };
}
