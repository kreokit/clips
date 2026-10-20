import { useEffect, useRef, useState } from "react";
import { Project, formatDuration } from "../lib/projects";
import {
  CANCELLED,
  LANGUAGES,
  MODEL_NAMES,
  ModelInfo,
  ProgressView,
  Transcript,
  TranscriptStatus,
  cancelJob,
  describeModelProgress,
  describeProgress,
  languageName,
  languageValue,
  listModels,
  modelHint,
  normalized,
  prepareModel,
  savedNote,
  startLabel,
  transcribe,
  transcriptStatus,
  withLanguage,
  wordCount,
} from "../lib/transcription";
import { useProject } from "../store/project";
import { Button, Checkbox, Field, ProgressBar, Segmented, Select, TextInput } from "../ui";

type Run = { kind: "idle" } | { kind: "running"; view: ProgressView } | { kind: "error"; message: string };

/** How long typing in "Custom words" waits before the cache is looked up again. */
const STATUS_DELAY_MS = 300;

/**
 * The Processing screen's transcription step: settings, then progress with cancel (a
 * cancelled run resumes), then the transcript. Milestone 3 adds the moment finder after it.
 */
export function TranscribePanel({ project, onRunningChange }: { project: Project; onRunningChange: (running: boolean) => void }) {
  const update = useProject((s) => s.update);
  const settings = project.transcription;
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [status, setStatus] = useState<TranscriptStatus | null>(null);
  const [run, setRun] = useState<Run>({ kind: "idle" });
  const [editing, setEditing] = useState(false);
  // The last run was cancelled (for "Nothing saved yet").
  const [cancelled, setCancelled] = useState(false);
  // Progress events can arrive after a run has ended; only the active run may update.
  const runId = useRef(0);
  const active = useRef(false);
  const running = run.kind === "running";
  const settingsKey = JSON.stringify(normalized(settings));

  useEffect(() => {
    listModels().then(setModels, (e) => setRun({ kind: "error", message: String(e) }));
  }, []);

  // Leaving the project stops a running job; its finished parts stay cached.
  useEffect(
    () => () => {
      if (active.current) {
        runId.current++;
        void cancelJob();
      }
    },
    [],
  );

  useEffect(() => onRunningChange(running), [running, onRunningChange]);

  useEffect(() => {
    if (running) return;
    let current = true;
    const timer = setTimeout(() => {
      transcriptStatus(project.id, normalized(settings)).then(
        (s) => current && setStatus(s),
        (e) => current && setRun({ kind: "error", message: String(e) }),
      );
    }, STATUS_DELAY_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
    // settingsKey stands for the settings' contents.
  }, [project.id, settingsKey, running]);

  const selected = models.find((m) => m.id === settings.model);
  const change = (patch: Partial<typeof settings>) => update({ transcription: { ...settings, ...patch } });

  async function start() {
    const id = ++runId.current;
    const show = (next: Run) => {
      if (runId.current === id) setRun(next);
    };
    const sent = normalized(settings);
    active.current = true;
    setCancelled(false);
    try {
      show({ kind: "running", view: { label: "Starting…", detail: null, percent: null } });
      if (selected?.source !== "ours") {
        await prepareModel(sent.model, (p) => show({ kind: "running", view: describeModelProgress(p, sent.model) }));
        listModels().then(setModels, () => {});
      }
      const transcript = await transcribe(project.id, sent, (p) => show({ kind: "running", view: describeProgress(p) }));
      if (runId.current === id) {
        setStatus((s) => ({ transcript, saved_until_ms: 0, parts: s?.parts ?? 1 }));
        setEditing(false);
        setRun({ kind: "idle" });
      }
    } catch (e) {
      const wasCancelled = String(e) === CANCELLED;
      if (wasCancelled && runId.current === id) setCancelled(true);
      show(wasCancelled ? { kind: "idle" } : { kind: "error", message: String(e) });
    } finally {
      if (runId.current === id) active.current = false;
    }
  }

  const transcript = status?.transcript ?? null;
  const note = savedNote(status, cancelled, project.video.duration_ms);
  if (transcript && !editing && !running) {
    return <TranscriptView transcript={transcript} model={settings.model} onChange={() => setEditing(true)} />;
  }

  return (
    <section className="panel" aria-label="Transcribe">
      <h2 className="panel-title">Transcribe</h2>
      {running ? (
        <div className="transcribe-progress" aria-live="polite">
          <ProgressBar value={run.view.percent} label="Transcription progress" />
          <span className="transcribe-label">{run.view.label}</span>
          {run.view.detail && <span className="transcribe-detail tabular">{run.view.detail}</span>}
          <p className="transcribe-note">
            You can cancel at any time: the finished text is saved about every 30 seconds, and Transcribe resumes from there.
            The computer stays awake meanwhile.
          </p>
          <div className="panel-actions">
            <Button variant="secondary" onClick={() => void cancelJob()}>
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <div className="transcribe-form">
          <Field label="Language">
            <Select
              label="Language"
              value={languageValue(settings)}
              onChange={(code) => update({ transcription: withLanguage(settings, code) })}
              options={LANGUAGES.map((l) => ({ value: l.code, label: l.label }))}
            />
          </Field>
          <Field label="Model">
            <Segmented
              label="Model"
              value={settings.model}
              onChange={(model) => change({ model })}
              options={models.map((m) => ({ value: m.id, label: MODEL_NAMES[m.id].label, hint: modelHint(m) }))}
            />
          </Field>
          <Field label="Custom words" hint="Names, brands or jargon, separated by commas. Helps Whisper spell them right.">
            <TextInput
              value={settings.custom_words}
              onChange={(e) => change({ custom_words: e.target.value })}
              placeholder="e.g. KreoKit, Dr. Okafor, TypeScript"
              spellCheck={false}
            />
          </Field>
          <Checkbox
            label="Hide sound tags like [MUSIC]"
            checked={settings.hide_non_speech}
            onChange={(hide_non_speech) => change({ hide_non_speech })}
          />
          {selected && selected.source === null && (
            <p className="transcribe-note">
              The {MODEL_NAMES[selected.id].label} model downloads once, then works offline.
            </p>
          )}
          <p className="transcribe-note">
            {formatDuration(project.video.duration_ms)} of video. Long videos take a while on a computer's processor; you'll
            see the time left once it has started.
          </p>
          {note && <p className="transcribe-saved">{note}</p>}
          {run.kind === "error" && <p className="transcribe-error">{run.message}</p>}
          <div className="panel-actions">
            {transcript && (
              <Button variant="secondary" onClick={() => setEditing(false)}>
                Back to transcript
              </Button>
            )}
            <Button variant="primary" onClick={() => void start()} disabled={!selected}>
              {startLabel(status)}
            </Button>
          </div>
        </div>
      )}
    </section>
  );
}

/** The finished transcript (read-only until milestone 3's clips board). */
function TranscriptView({ transcript, model, onChange }: { transcript: Transcript; model: keyof typeof MODEL_NAMES; onChange: () => void }) {
  const words = wordCount(transcript);
  return (
    <section className="panel" aria-label="Transcript">
      <div className="panel-head">
        <h2 className="panel-title">Transcript</h2>
        <span className="panel-meta tabular">
          {languageName(transcript.language)} · {words.toLocaleString("en-US")} words · {MODEL_NAMES[model].label} model
        </span>
        <Button variant="ghost" size="sm" onClick={onChange}>
          Change settings
        </Button>
      </div>
      {transcript.segments.length === 0 ? (
        <p className="transcribe-note">No speech was found in this video.</p>
      ) : (
        <ol className="transcript-list">
          {transcript.segments.map((s) => (
            <li key={s.id}>
              <span className="transcript-time tabular">{formatDuration(s.start_ms)}</span>
              {s.text ? <span>{s.text}</span> : <span className="transcript-sound">{(s.sounds ?? []).map((t) => t.text).join(" ")}</span>}
            </li>
          ))}
        </ol>
      )}
      <p className="transcribe-note">Next: finding the best moments (coming in the next build).</p>
    </section>
  );
}
