import { useEffect, useRef } from "react";
import { AlertTriangle, Info, RotateCcw, ScanFace, Trash2 } from "lucide-react";
import type { CropMode, Framing } from "../lib/clips";
import type { PathState } from "../lib/faces";
import { Axis, CROP_MODES, keyframeNear } from "../lib/reframe";
import { Button, ProgressBar, Segmented } from "../ui";

/** Overview width in CSS pixels (its height follows the video's aspect). */
const OVERVIEW_WIDTH = 312;

/**
 * How the clip becomes 9:16: the mode, a small view of the whole frame with the crop
 * window (drag it to set the crop at the playhead: Manual), and what the face analysis
 * is doing.
 */
export function FramingPanel({
  framing,
  pathState,
  video,
  source,
  axis,
  span,
  pos,
  currentMs,
  frameKey,
  lowRes,
  onMode,
  onDrag,
  onRemoveKeyframe,
  onRetry,
}: {
  framing: Framing;
  pathState: PathState;
  /** The editor's video, drawn into the overview. */
  video: HTMLVideoElement | null;
  /** Source display size. */
  source: { width: number; height: number };
  axis: Axis;
  /** Crop size on the axis, as a share of the source. */
  span: number;
  /** Crop centre now (null: Fit + blur, no crop). */
  pos: number | null;
  currentMs: number;
  /** Changes whenever the shown frame may have changed (redraws the overview). */
  frameKey: string;
  lowRes: boolean;
  onMode: (mode: CropMode) => void;
  /** The crop dragged to `pos` at the playhead. */
  onDrag: (pos: number) => void;
  onRemoveKeyframe: () => void;
  onRetry: () => void;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const dragging = useRef(false);
  const height = Math.round((OVERVIEW_WIDTH * source.height) / Math.max(source.width, 1));

  // The current frame, dimmed outside the crop, with the crop's outline.
  useEffect(() => {
    const el = canvas.current;
    const ctx = el?.getContext("2d");
    if (!el || !ctx) return;
    const dpr = window.devicePixelRatio || 1;
    el.width = Math.round(OVERVIEW_WIDTH * dpr);
    el.height = Math.round(height * dpr);
    ctx.scale(dpr, dpr);
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, OVERVIEW_WIDTH, height);
    if (video && video.readyState >= 2) ctx.drawImage(video, 0, 0, OVERVIEW_WIDTH, height);
    if (pos === null) return;
    const css = getComputedStyle(el);
    const [x, y, w, h] =
      axis === "x"
        ? [(pos - span / 2) * OVERVIEW_WIDTH, 0, span * OVERVIEW_WIDTH, height]
        : [0, (pos - span / 2) * height, OVERVIEW_WIDTH, span * height];
    ctx.fillStyle = "rgba(0, 0, 0, 0.55)";
    ctx.fillRect(0, 0, OVERVIEW_WIDTH, height);
    if (video && video.readyState >= 2) {
      const sx = (x / OVERVIEW_WIDTH) * video.videoWidth;
      const sy = (y / height) * video.videoHeight;
      const sw = (w / OVERVIEW_WIDTH) * video.videoWidth;
      const sh = (h / height) * video.videoHeight;
      ctx.drawImage(video, sx, sy, sw, sh, x, y, w, h);
    }
    ctx.strokeStyle = css.getPropertyValue("--color-accent").trim() || "#c8f135";
    ctx.lineWidth = 2;
    ctx.strokeRect(x + 1, y + 1, w - 2, h - 2);
  }, [video, pos, axis, span, height, frameKey]);

  const posAt = (e: React.PointerEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    return axis === "x" ? (e.clientX - r.left) / r.width : (e.clientY - r.top) / r.height;
  };

  const status = (() => {
    if (framing.mode === "fit_blur" || framing.mode === "center") return null;
    switch (pathState.kind) {
      case "finding":
        return (
          <div className="framing-status">
            <span>
              <ScanFace size={14} strokeWidth={2} aria-hidden /> Finding faces…
            </span>
            <ProgressBar value={pathState.progress === null ? null : Math.round(pathState.progress * 100)} label="Finding faces" />
          </div>
        );
      case "waiting":
        return (
          <p className="framing-status">
            <Info size={14} strokeWidth={2} aria-hidden /> Faces are found once the transcription has finished.
          </p>
        );
      case "error":
        return (
          <div className="framing-status framing-error" role="alert">
            <span>
              <AlertTriangle size={14} strokeWidth={2} aria-hidden /> Couldn't find faces: {pathState.message}
            </span>
            <Button variant="secondary" size="sm" onClick={onRetry}>
              Try again
            </Button>
          </div>
        );
      case "ready":
        return framing.mode === "track" && pathState.path.no_face ? (
          <div className="framing-status" role="status">
            <span>
              <Info size={14} strokeWidth={2} aria-hidden /> No face found, using center crop
            </span>
            <Button variant="secondary" size="sm" onClick={() => onMode("fit_blur")}>
              Use Fit + blur
            </Button>
          </div>
        ) : null;
    }
  })();

  const near = framing.mode === "manual" ? keyframeNear(framing.manual, currentMs) : undefined;

  return (
    <aside className="framing" aria-label="Framing">
      <Segmented label="Crop" value={framing.mode} options={CROP_MODES} onChange={onMode} />
      <canvas
        ref={canvas}
        className={["framing-overview", pos !== null && "draggable"].filter(Boolean).join(" ")}
        style={{ width: OVERVIEW_WIDTH, height }}
        aria-label="The whole frame: drag the crop to set it at the playhead"
        onPointerDown={(e) => {
          if (e.button !== 0 || pos === null) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          dragging.current = true;
          onDrag(posAt(e));
        }}
        onPointerMove={(e) => dragging.current && onDrag(posAt(e))}
        onPointerUp={() => (dragging.current = false)}
        onPointerCancel={() => (dragging.current = false)}
      />
      <p className="framing-hint">
        {framing.mode === "fit_blur"
          ? "The whole frame, over a blurred copy of itself."
          : framing.mode === "manual"
            ? "Drag the crop to set it at the playhead; the crop moves smoothly between your points."
            : "Drag the crop to adjust it at the playhead (switches to Manual)."}
      </p>
      {framing.mode === "manual" && (
        <div className="framing-actions">
          <Button variant="ghost" size="sm" icon={Trash2} disabled={!near} onClick={onRemoveKeyframe}>
            Remove point
          </Button>
          <Button variant="ghost" size="sm" icon={RotateCcw} onClick={() => onMode("track")}>
            Back to Follow face
          </Button>
        </div>
      )}
      {status}
      {lowRes && (
        <p className="framing-status framing-note">
          <Info size={14} strokeWidth={2} aria-hidden /> Low-resolution source; the vertical clip will look soft
        </p>
      )}
    </aside>
  );
}

