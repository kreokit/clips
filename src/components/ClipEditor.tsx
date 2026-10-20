import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  ChevronLeft,
  ChevronRight,
  Info,
  Magnet,
  Maximize2,
  Pause,
  Play,
  Plus,
  RotateCcw,
  Trash2,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import {
  EdgeKind,
  Range,
  clipAround,
  clipFromWords,
  clipView,
  clipWords,
  contextWords,
  dragClip,
  hitEdge,
  lengthNote,
  sentenceMarks,
  snapEdge,
  snapTargets,
  wordIndexAt,
} from "../lib/clipEdit";
import { Clip, SpokenText, applyScores, manualClip, scoreClips, spokenText, tiktokWarning } from "../lib/clips";
import { Project, formatDuration } from "../lib/projects";
import { SNAP_PX, View, clampView, follow, msAt, tickLabel, tickStep, xAt, zoomAt } from "../lib/timeline";
import { normalized } from "../lib/transcription";
import { useProject } from "../store/project";
import { Button, IconButton, Popover, toast } from "../ui";
import { WhyThisClip } from "./ClipsBoard";
import "../styles/editor.css";

/** Peaks per second from `waveform_range` (src-tauri/src/waveform.rs). */
const PEAKS_PER_SECOND = 100;
/** Waveform is read this far beyond the visible window, so small pans need no new read. */
const WAVEFORM_MARGIN_MS = 30_000;
/** A press that moves less than this is a click, not a drag. */
const CLICK_PX = 3;

/**
 * Edits one clip: the source video (16:9 here; the 9:16 preview comes with reframing),
 * the clip's words with a sentence of context, and a timeline with in/out handles that
 * snap to sentence and word edges. New clips: at the playhead (N) or from selected words.
 * Keys: Space plays the clip, I / O set the in / out point at the playhead, [ / ] go to
 * the previous / next clip, K keeps, X discards, Escape goes back to the board.
 */
export function ClipEditor({
  project,
  clipId,
  order,
  onOpen,
  onClose,
}: {
  project: Project;
  clipId: number;
  /** The board's order, for previous / next. */
  order: number[];
  /** Opens another clip; `order` replaces the order when clips were added or removed. */
  onOpen: (id: number, order?: number[]) => void;
  onClose: () => void;
}) {
  const update = useProject((s) => s.update);
  const [text, setText] = useState<SpokenText | null>(null);
  const [draft, setDraft] = useState<Range | null>(null);
  const [currentMs, setCurrentMs] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [videoError, setVideoError] = useState(false);
  const [why, setWhy] = useState(false);
  const whyButton = useRef<HTMLButtonElement>(null);
  const video = useRef<HTMLVideoElement>(null);
  // While "Play clip" runs, playback stops at the out point.
  const stopAt = useRef<number | null>(null);

  const clip = project.clips.find((c) => c.id === clipId);
  const durationMs = project.video.duration_ms;
  const index = order.indexOf(clipId);
  const shown: Range | null = draft ?? (clip ? { in_ms: clip.in_ms, out_ms: clip.out_ms } : null);

  useEffect(() => {
    let live = true;
    spokenText(project).then(
      (t) => live && setText(t),
      (e) => toast(String(e), "error"),
    );
    return () => {
      live = false;
    };
    // The transcript settings decide which text.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project.id, JSON.stringify(project.transcription)]);

  const seek = useCallback(
    (ms: number) => {
      const el = video.current;
      const t = Math.min(Math.max(ms, 0), durationMs);
      if (el) el.currentTime = t / 1000;
      setCurrentMs(t);
    },
    [durationMs],
  );

  // A new clip starts at its in point, paused.
  useEffect(() => {
    if (!clip) return;
    video.current?.pause();
    stopAt.current = null;
    setDraft(null);
    seek(clip.in_ms);
    // Only when another clip opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clipId]);

  // Follow playback per animation frame (timeupdate is too coarse), stopping at the out point.
  useEffect(() => {
    const el = video.current;
    if (!el) return;
    let raf = 0;
    const tick = () => {
      const ms = Math.round(el.currentTime * 1000);
      if (stopAt.current !== null && ms >= stopAt.current) {
        el.pause();
        el.currentTime = stopAt.current / 1000;
        stopAt.current = null;
      }
      setCurrentMs(Math.round(el.currentTime * 1000));
      if (!el.paused) raf = requestAnimationFrame(tick);
    };
    const onPlay = () => {
      setPlaying(true);
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(tick);
    };
    const onPause = () => {
      setPlaying(false);
      tick();
    };
    el.addEventListener("play", onPlay);
    el.addEventListener("pause", onPause);
    el.addEventListener("seeked", tick);
    return () => {
      cancelAnimationFrame(raf);
      el.removeEventListener("play", onPlay);
      el.removeEventListener("pause", onPause);
      el.removeEventListener("seeked", tick);
    };
  }, []);

  /** Plays the clip from its in point (or from the playhead when inside it) to its out point. */
  const playClip = useCallback(() => {
    const el = video.current;
    if (!el || !shown) return;
    if (!el.paused) {
      el.pause();
      return;
    }
    const now = el.currentTime * 1000;
    if (now < shown.in_ms || now >= shown.out_ms - 50) el.currentTime = shown.in_ms / 1000;
    stopAt.current = shown.out_ms;
    void el.play();
  }, [shown]);

  /** Saves new in/out points and scores the clip again. */
  const commit = useCallback(
    async (range: Range) => {
      setDraft(null);
      const p = useProject.getState().project;
      const current = p?.clips.find((c) => c.id === clipId);
      if (!p || !current || (range.in_ms === current.in_ms && range.out_ms === current.out_ms)) return;
      const moved: Clip = { ...current, ...range, edited: current.source === "found" ? true : current.edited };
      update({ clips: p.clips.map((c) => (c.id === clipId ? moved : c)) });
      try {
        const [moment] = await scoreClips(p.id, normalized(p.transcription), p.finder, [[range.in_ms, range.out_ms]]);
        const latest = useProject.getState().project;
        if (latest?.id === p.id) update({ clips: applyScores(latest.clips, [moved], [moment]) });
      } catch (e) {
        toast(`Couldn't score the clip: ${String(e)}`, "error");
      }
    },
    [clipId, update],
  );

  const setStatus = useCallback(
    (status: Clip["status"]) => {
      const p = useProject.getState().project;
      if (!p) return;
      update({ clips: p.clips.map((c) => (c.id === clipId ? { ...c, status: c.status === status ? "proposed" : status } : c)) });
    },
    [clipId, update],
  );

  /** Adds a clip made by hand (kept, scored) and opens it. */
  const createClip = useCallback(
    async (range: Range | null) => {
      const p = useProject.getState().project;
      if (!range || !p) return;
      let moment = null;
      try {
        [moment] = await scoreClips(p.id, normalized(p.transcription), p.finder, [[range.in_ms, range.out_ms]]);
      } catch (e) {
        toast(`Couldn't score the clip: ${String(e)}`, "error");
      }
      const latest = useProject.getState().project;
      if (latest?.id !== p.id) return;
      const added = manualClip(latest.clips, range.in_ms, range.out_ms, moment);
      update({ clips: [...latest.clips, added] });
      onOpen(added.id, [...order, added.id]);
    },
    [order, onOpen, update],
  );

  const newClipHere = useCallback(() => {
    if (!text) return;
    const ms = video.current ? video.current.currentTime * 1000 : currentMs;
    void createClip(clipAround(text, ms, project.finder, durationMs));
  }, [text, currentMs, project.finder, durationMs, createClip]);

  /** Removes a clip made by hand and opens the next one (or goes back). */
  const remove = useCallback(() => {
    const p = useProject.getState().project;
    if (!p) return;
    update({ clips: p.clips.filter((c) => c.id !== clipId) });
    const rest = order.filter((id) => id !== clipId);
    const next = order[index + 1] ?? order[index - 1];
    if (next !== undefined) onOpen(next, rest);
    else onClose();
  }, [clipId, order, index, onOpen, onClose, update]);

  // Keys, unless typing or on a button (Space would press it).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || !shown || !text) return;
      const target = e.target as HTMLElement;
      if (target.closest("input, textarea, select, [contenteditable]")) return;
      if (e.code === "Space") {
        if (target.closest("button")) return;
        playClip();
      } else if (e.key === "i" || e.key === "o") {
        const { ins, outs } = snapTargets(text, durationMs);
        const ms = Math.round(video.current ? video.current.currentTime * 1000 : currentMs);
        const range =
          e.key === "i"
            ? dragClip(shown, "in", snapEdge(ms, ins, 300) - shown.in_ms, { durationMs, ins, outs, snapMs: 0 })
            : dragClip(shown, "out", snapEdge(ms, outs, 300) - shown.out_ms, { durationMs, ins, outs, snapMs: 0 });
        void commit(range);
      } else if (e.key === "[" && index > 0) onOpen(order[index - 1]);
      else if (e.key === "]" && index >= 0 && index < order.length - 1) onOpen(order[index + 1]);
      else if (e.key === "n") newClipHere();
      else if (e.key === "k") setStatus("kept");
      else if (e.key === "x") setStatus("discarded");
      else if (e.key === "Escape") onClose();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [shown, text, durationMs, currentMs, index, order, onOpen, onClose, playClip, commit, setStatus, newClipHere]);

  if (!clip || !shown) return null;
  const length = shown.out_ms - shown.in_ms;
  const note = lengthNote(length, project.finder);
  const tiktok = tiktokWarning(length, project.finder) !== null;
  const m = clip.moment;

  return (
    <div className="editor">
      <header className="editor-head">
        <Button variant="ghost" size="sm" icon={ArrowLeft} onClick={onClose}>
          Clips
        </Button>
        <div className="editor-nav">
          <IconButton
            icon={ChevronLeft}
            label="Previous clip"
            shortcut="["
            size="sm"
            disabled={index <= 0}
            onClick={() => onOpen(order[index - 1])}
          />
          <span className="editor-count tabular">
            Clip {index + 1} of {order.length}
          </span>
          <IconButton
            icon={ChevronRight}
            label="Next clip"
            shortcut="]"
            size="sm"
            disabled={index < 0 || index >= order.length - 1}
            onClick={() => onOpen(order[index + 1])}
          />
        </div>
        <span className="editor-length tabular">
          {formatDuration(shown.in_ms)}–{formatDuration(shown.out_ms)} · {formatDuration(length)}
          {note && (
            <span className={["editor-warning", tiktok && "tiktok"].filter(Boolean).join(" ")} role={tiktok ? "alert" : undefined}>
              {tiktok && <AlertTriangle size={14} strokeWidth={2} aria-hidden />}
              {note}
            </span>
          )}
        </span>
        <span className="editor-spacer" />
        <Button variant="ghost" size="sm" icon={Plus} onClick={newClipHere} disabled={!text} title="A new clip at the playhead (N)">
          New clip here
        </Button>
        {m && (
          <>
            <Button ref={whyButton} variant="ghost" size="sm" icon={Info} onClick={() => setWhy((w) => !w)} aria-expanded={why}>
              Score {m.score}
            </Button>
            <Popover open={why} onClose={() => setWhy(false)} anchor={whyButton} align="end" className="why-popover">
              <WhyThisClip moment={m} />
            </Popover>
          </>
        )}
        {clip.source === "manual" ? (
          <Button variant="secondary" size="sm" icon={Trash2} onClick={remove}>
            Delete
          </Button>
        ) : clip.status === "discarded" ? (
          <Button variant="secondary" size="sm" icon={RotateCcw} onClick={() => setStatus("discarded")}>
            Restore
          </Button>
        ) : (
          <>
            <Button variant="secondary" size="sm" icon={X} onClick={() => setStatus("discarded")}>
              Discard
            </Button>
            <Button variant="secondary" size="sm" icon={Check} aria-pressed={clip.status === "kept"} onClick={() => setStatus("kept")}>
              {clip.status === "kept" ? "Kept" : "Keep"}
            </Button>
          </>
        )}
      </header>

      <div className="editor-main">
        <div className="editor-player">
          <div className="editor-stage">
            <video
              ref={video}
              src={convertFileSrc(project.video_path)}
              playsInline
              preload="auto"
              onClick={playClip}
              onLoadedMetadata={() => {
                setVideoError(false);
                seek(clip.in_ms);
              }}
              onError={() => setVideoError(true)}
            />
            {videoError && (
              <p className="editor-video-error">
                This video can't be previewed here. MP4 and MOV (H.264) play best; the clips are still found and exported from the original.
              </p>
            )}
          </div>
          <div className="editor-controls">
            <IconButton icon={playing ? Pause : Play} label={playing ? "Pause" : "Play clip"} shortcut="Space" onClick={playClip} />
            <span className="editor-time tabular">
              {formatDuration(Math.max(0, currentMs - shown.in_ms))} / {formatDuration(length)}
            </span>
            <span className="editor-hint">I / O set the in / out point at the playhead</span>
          </div>
        </div>
        <ClipWords
          text={text}
          clip={shown}
          currentMs={currentMs}
          onSeek={seek}
          onMake={(a, b) => text && void createClip(clipFromWords(text, a, b, durationMs))}
        />
      </div>

      <ClipTimeline
        projectId={project.id}
        clipKey={clipId}
        durationMs={durationMs}
        clip={shown}
        text={text}
        currentMs={currentMs}
        playing={playing}
        onSeek={seek}
        onDraft={setDraft}
        onCommit={(r) => void commit(r)}
      />
    </div>
  );
}

/**
 * The clip's words with a sentence of context; the spoken word is highlighted, a click
 * seeks, and dragging across words offers to make a clip of their sentences.
 */
function ClipWords({
  text,
  clip,
  currentMs,
  onSeek,
  onMake,
}: {
  text: SpokenText | null;
  clip: Range;
  currentMs: number;
  onSeek: (ms: number) => void;
  onMake: (from: number, to: number) => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  const [selection, setSelection] = useState<[number, number] | null>(null);
  const [selecting, setSelecting] = useState(false);
  const range = text ? contextWords(text, clip) : null;
  const inside = text ? clipWords(text.words, clip) : null;
  const spoken = text ? wordIndexAt(text.words, currentMs) : -1;
  // The spoken word, lingering 0.2 s after it ends; a word outside the clip only while
  // the playhead is outside it too (not during the clip's lead-in).
  const inClipNow = currentMs >= clip.in_ms && currentMs < clip.out_ms;
  const spokenInside = inside !== null && spoken >= inside[0] && spoken <= inside[1];
  const current = text && spoken >= 0 && currentMs < text.words[spoken].end_ms + 200 && (spokenInside || !inClipNow) ? spoken : -1;

  useEffect(() => {
    list.current?.querySelector(".word.current")?.scrollIntoView({ block: "nearest" });
  }, [current]);

  // A press on one word seeks; a drag across words selects them.
  useEffect(() => {
    if (!selecting) return;
    const onUp = () => {
      setSelecting(false);
      if (selection && selection[0] === selection[1] && text) {
        onSeek(text.words[selection[0]].start_ms);
        setSelection(null);
      }
    };
    window.addEventListener("pointerup", onUp);
    return () => window.removeEventListener("pointerup", onUp);
  }, [selecting, selection, text, onSeek]);

  if (!text) return <div className="editor-words muted">Reading the transcript…</div>;
  if (!range) return <div className="editor-words muted">No words here.</div>;
  const sentences = text.sentences.filter((s) => s.last >= range[0] && s.first <= range[1]);
  const [lo, hi] = selection ? [Math.min(...selection), Math.max(...selection)] : [-1, -1];
  return (
    <div className="editor-side">
      {selection && lo !== hi && !selecting && (
        <div className="editor-selection">
          <span>Make a clip of these sentences?</span>
          <Button
            variant="primary"
            size="sm"
            icon={Plus}
            onClick={() => {
              onMake(lo, hi);
              setSelection(null);
            }}
          >
            Make clip
          </Button>
          <IconButton icon={X} label="Clear the selection" size="sm" onClick={() => setSelection(null)} />
        </div>
      )}
      <div className="editor-words" ref={list} aria-label="Clip text">
        {sentences.map((s) => (
          <p key={s.first} className="editor-sentence">
            {text.words.slice(s.first, s.last + 1).map((w, k) => {
              const i = s.first + k;
              const inClip = inside !== null && i >= inside[0] && i <= inside[1];
              const cls = ["word", !inClip && "outside", i === current && "current", i >= lo && i <= hi && "selected"];
              return (
                <span
                  key={i}
                  className={cls.filter(Boolean).join(" ")}
                  onPointerDown={(e) => {
                    if (e.button !== 0) return;
                    e.preventDefault();
                    setSelecting(true);
                    setSelection([i, i]);
                  }}
                  onPointerEnter={() => selecting && setSelection((sel) => (sel ? [sel[0], i] : [i, i]))}
                >
                  {w.text}{" "}
                </span>
              );
            })}
            {s.ends === "pause" || s.ends === "weak_pause" ? <span className="editor-pause" title="A pause, no punctuation here" /> : null}
          </p>
        ))}
      </div>
    </div>
  );
}

interface Drag {
  kind: EdgeKind;
  startX: number;
  original: Range;
  moved: boolean;
}

/** The clip on a timeline with ruler, waveform, sentence marks and in/out handles. */
function ClipTimeline({
  projectId,
  clipKey,
  durationMs,
  clip,
  text,
  currentMs,
  playing,
  onSeek,
  onDraft,
  onCommit,
}: {
  projectId: string;
  clipKey: number;
  durationMs: number;
  clip: Range;
  text: SpokenText | null;
  currentMs: number;
  playing: boolean;
  onSeek: (ms: number) => void;
  onDraft: (r: Range | null) => void;
  onCommit: (r: Range) => void;
}) {
  const body = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [view, setView] = useState<View>({ startMs: 0, msPerPx: 50 });
  const [snapping, setSnapping] = useState(true);
  const [wave, setWave] = useState<{ from: number; to: number; peaks: Uint8Array } | null>(null);
  const drag = useRef<Drag | null>(null);
  const scrubbing = useRef(false);
  const latestDraft = useRef<Range | null>(null);
  const framedFor = useRef<number | null>(null);

  useLayoutEffect(() => {
    const el = body.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // Frame each clip when it opens.
  useEffect(() => {
    if (!width || framedFor.current === clipKey) return;
    framedFor.current = clipKey;
    setView(clipView(clip, width, durationMs));
  }, [width, clipKey, clip, durationMs]);

  useEffect(() => {
    if (playing && width) setView((v) => follow(v, currentMs, durationMs, width));
  }, [playing, currentMs, width, durationMs]);

  // Read the waveform for the visible window (plus a margin) when it isn't loaded yet.
  const visibleTo = view.startMs + width * view.msPerPx;
  useEffect(() => {
    if (!width) return;
    if (wave && wave.from <= view.startMs && wave.to >= Math.min(visibleTo, durationMs)) return;
    const from = Math.max(0, Math.round(view.startMs - WAVEFORM_MARGIN_MS));
    const to = Math.min(durationMs, Math.round(visibleTo + WAVEFORM_MARGIN_MS));
    let live = true;
    const timer = setTimeout(() => {
      invoke<ArrayBuffer>("waveform_range", { id: projectId, fromMs: from, toMs: to }).then(
        (buffer) => live && setWave({ from, to, peaks: new Uint8Array(buffer) }),
        () => {},
      );
    }, 150);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [projectId, view.startMs, visibleTo, width, durationMs, wave]);

  useEffect(() => {
    const el = body.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!width) return;
      e.preventDefault();
      const x = e.clientX - el.getBoundingClientRect().left;
      if (e.ctrlKey || e.metaKey) setView((v) => zoomAt(v, Math.exp(-e.deltaY * 0.01), x, durationMs, width));
      else {
        const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
        setView((v) => clampView({ ...v, startMs: v.startMs + delta * v.msPerPx }, durationMs, width));
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [durationMs, width]);

  const localX = (clientX: number) => clientX - (body.current?.getBoundingClientRect().left ?? 0);
  const seekToX = (x: number) => onSeek(Math.round(Math.min(Math.max(msAt(x, view), 0), durationMs)));
  const left = xAt(clip.in_ms, view);
  const blockWidth = (clip.out_ms - clip.in_ms) / view.msPerPx;

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const x = localX(e.clientX);
    if ((e.target as HTMLElement).closest("[data-clip-block]")) {
      drag.current = { kind: hitEdge(x, left, blockWidth), startX: x, original: clip, moved: false };
    } else {
      scrubbing.current = true;
      seekToX(x);
    }
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const x = localX(e.clientX);
    if (scrubbing.current) return seekToX(x);
    const d = drag.current;
    if (!d || !text) return;
    if (!d.moved && Math.abs(x - d.startX) < CLICK_PX) return;
    d.moved = true;
    const { ins, outs } = snapTargets(text, durationMs);
    const next = dragClip(d.original, d.kind, (x - d.startX) * view.msPerPx, {
      durationMs,
      ins,
      outs,
      snapMs: snapping && !e.altKey ? SNAP_PX * view.msPerPx : 0,
    });
    latestDraft.current = next;
    onDraft(next);
    if (d.kind !== "move") onSeek(d.kind === "in" ? next.in_ms : next.out_ms);
  };

  const onPointerUp = () => {
    const d = drag.current;
    drag.current = null;
    scrubbing.current = false;
    if (!d) return;
    if (d.moved && latestDraft.current) onCommit(latestDraft.current);
    else onSeek(d.original.in_ms);
    latestDraft.current = null;
  };

  const zoomBy = (factor: number) => setView((v) => zoomAt(v, factor, xAt(currentMs, v), durationMs, width));
  const step = tickStep(view.msPerPx);
  const ticks: number[] = [];
  for (let t = Math.floor(view.startMs / step) * step; t <= visibleTo; t += step) if (t >= 0) ticks.push(t);
  const marks = text ? sentenceMarks(text).filter((m) => m.ms >= view.startMs && m.ms <= visibleTo) : [];
  const playheadX = xAt(currentMs, view);

  return (
    <section className="timeline" aria-label="Timeline">
      <div className="timeline-toolbar">
        <IconButton icon={Maximize2} label="Frame the clip" size="sm" onClick={() => setView(clipView(clip, width, durationMs))} />
        <IconButton icon={ZoomOut} label="Zoom out" shortcut="⌘ scroll" size="sm" onClick={() => zoomBy(1 / 1.5)} />
        <IconButton icon={ZoomIn} label="Zoom in" shortcut="⌘ scroll" size="sm" onClick={() => zoomBy(1.5)} />
        <span className="timeline-divider" />
        <IconButton
          icon={Magnet}
          label={snapping ? "Snapping to sentences and words (hold ⌥ to drag freely)" : "Snapping off"}
          size="sm"
          pressed={snapping}
          onClick={() => setSnapping((s) => !s)}
        />
        <span className="timeline-legend">
          <span className="mark-sample punctuation" /> sentence end <span className="mark-sample pause" /> pause, no punctuation
        </span>
        <span className="timeline-hint">Drag the edges to trim · drag the clip to move it</span>
      </div>
      <div
        ref={body}
        className="timeline-body"
        tabIndex={0}
        aria-label="Timeline: click to seek, drag the clip's edges to trim"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        <div className="timeline-ruler">
          {ticks.map((t) => (
            <span key={t} className="timeline-tick" style={{ left: xAt(t, view) }}>
              {tickLabel(t, step)}
            </span>
          ))}
        </div>
        <Waveform wave={wave} view={view} width={width} clip={clip} />
        {marks.map((m) => (
          <span key={m.ms} className={`timeline-mark ${m.kind}`} style={{ left: xAt(m.ms, view) }} />
        ))}
        <div className="timeline-track">
          <div data-clip-block className="timeline-block clip-block" style={{ left, width: Math.max(blockWidth, 4) }}>
            <span className="timeline-block-edge start" />
            <span className="timeline-block-text">{formatDuration(clip.out_ms - clip.in_ms)}</span>
            <span className="timeline-block-edge end" />
          </div>
        </div>
        {playheadX >= 0 && playheadX <= width && <div className="timeline-playhead" style={{ left: playheadX }} />}
      </div>
    </section>
  );
}

/** Audio peaks for the visible window; brighter inside the clip. */
function Waveform({
  wave,
  view,
  width,
  clip,
}: {
  wave: { from: number; peaks: Uint8Array } | null;
  view: View;
  width: number;
  clip: Range;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const HEIGHT = 56;
  useEffect(() => {
    const el = canvas.current;
    const ctx = el?.getContext("2d");
    if (!el || !ctx || !width) return;
    const dpr = window.devicePixelRatio || 1;
    el.width = Math.round(width * dpr);
    el.height = HEIGHT * dpr;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, width, HEIGHT);
    if (!wave) return;
    const css = getComputedStyle(el);
    const base = css.getPropertyValue("--color-waveform").trim();
    const active = css.getPropertyValue("--color-text-2").trim();
    const perMs = PEAKS_PER_SECOND / 1000;
    const mid = HEIGHT / 2;
    for (let x = 0; x < width; x++) {
      const from = Math.floor((msAt(x, view) - wave.from) * perMs);
      const to = Math.max(from + 1, Math.floor((msAt(x + 1, view) - wave.from) * perMs));
      let level = 0;
      for (let i = Math.max(from, 0); i < Math.min(to, wave.peaks.length); i++) level = Math.max(level, wave.peaks[i]);
      if (level === 0) continue;
      const h = Math.max(1, (level / 255) * (HEIGHT - 4));
      const t = msAt(x, view);
      ctx.fillStyle = t >= clip.in_ms && t < clip.out_ms ? active : base;
      ctx.fillRect(x, mid - h / 2, 1, h);
    }
  }, [wave, view, width, clip]);
  return <canvas ref={canvas} className="timeline-waveform" style={{ width, height: HEIGHT }} aria-hidden />;
}
