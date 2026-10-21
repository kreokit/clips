/**
 * One queue for face analysis (src-tauri/src/analysis.rs runs one job at a time, never
 * beside a transcription). The clip open in the editor goes first ("now"); kept clips are
 * analysed in the background. A "now" request stops a running job, which goes back in the
 * queue (its finished blocks are kept). While a transcription runs, the queue waits and
 * tries again.
 */

export type Priority = "now" | "background";
export type Range = [number, number];
type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/** analyze_faces' errors (lib.rs). */
const TRANSCRIBING = "transcribing";
const CANCELLED = "cancelled";

export interface QueueStatus {
  /** The job running now. */
  running: { projectId: string; ranges: Range[]; priority: Priority } | null;
  /** Jobs waiting behind it. */
  pending: number;
  /** Waiting for a transcription to finish. */
  waitingForTranscription: boolean;
}

interface Job {
  projectId: string;
  ranges: Range[];
  priority: Priority;
  key: string;
  waiters: { resolve: () => void; reject: (e: unknown) => void }[];
}

export class Dropped extends Error {
  constructor() {
    super("dropped");
  }
}

export class FaceQueue {
  private pending: Job[] = [];
  private running: Job | null = null;
  /** The running job was stopped to let a "now" job go first. */
  private preempted = false;
  private waiting = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private listeners = new Set<(s: QueueStatus) => void>();

  constructor(
    private invoke: Invoke,
    private retryMs = 5000,
  ) {}

  /** Resolves when the ranges are analysed; rejects with `Dropped` if cleared first. */
  request(projectId: string, ranges: Range[], priority: Priority): Promise<void> {
    const key = `${projectId}:${JSON.stringify(ranges)}`;
    return new Promise((resolve, reject) => {
      const waiter = { resolve, reject };
      const same = this.running?.key === key ? this.running : this.pending.find((j) => j.key === key);
      if (same) {
        same.waiters.push(waiter);
        if (priority === "now" && same.priority !== "now") this.promote(same);
      } else {
        const job: Job = { projectId, ranges, priority, key, waiters: [waiter] };
        if (priority === "now") {
          this.pending.unshift(job);
          this.preemptRunning();
        } else this.pending.push(job);
      }
      this.changed();
      this.pump();
    });
  }

  /** Drops every job (of one project, or all) and stops the running one. */
  clear(projectId?: string) {
    const drop = (j: Job) => projectId === undefined || j.projectId === projectId;
    for (const job of this.pending.filter(drop)) job.waiters.forEach((w) => w.reject(new Dropped()));
    this.pending = this.pending.filter((j) => !drop(j));
    if (this.running && drop(this.running)) {
      this.running.waiters.forEach((w) => w.reject(new Dropped()));
      this.running.waiters = [];
      void this.invoke("cancel_faces");
    }
    this.changed();
  }

  /** A transcription just finished: try now instead of at the next retry. */
  retrySoon() {
    if (!this.waiting) return;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.waiting = false;
    this.pump();
  }

  subscribe(listener: (s: QueueStatus) => void): () => void {
    this.listeners.add(listener);
    listener(this.status());
    return () => this.listeners.delete(listener);
  }

  status(): QueueStatus {
    const r = this.running;
    return {
      running: r ? { projectId: r.projectId, ranges: r.ranges, priority: r.priority } : null,
      pending: this.pending.length,
      waitingForTranscription: this.waiting,
    };
  }

  private promote(job: Job) {
    job.priority = "now";
    if (job !== this.running) {
      this.pending = [job, ...this.pending.filter((j) => j !== job)];
      this.preemptRunning();
    }
  }

  private preemptRunning() {
    if (this.running && this.running.priority === "background" && !this.preempted) {
      this.preempted = true;
      void this.invoke("cancel_faces");
    }
  }

  private changed() {
    const s = this.status();
    this.listeners.forEach((l) => l(s));
  }

  private pump() {
    if (this.running || this.waiting || this.pending.length === 0) return;
    const job = this.pending.shift()!;
    this.running = job;
    this.preempted = false;
    this.changed();
    this.invoke("analyze_faces", { id: job.projectId, ranges: job.ranges }).then(
      () => {
        this.running = null;
        job.waiters.forEach((w) => w.resolve());
        this.changed();
        this.pump();
      },
      (e: unknown) => {
        this.running = null;
        const message = String(e);
        if (job.waiters.length === 0) {
          // Cleared while running.
        } else if (message === CANCELLED && this.preempted) {
          // Stopped for a "now" job: back in the queue, behind the "now" jobs.
          const firstBackground = this.pending.findIndex((j) => j.priority === "background");
          this.pending.splice(firstBackground < 0 ? this.pending.length : firstBackground, 0, job);
        } else if (message === TRANSCRIBING) {
          this.pending.unshift(job);
          this.waiting = true;
          this.timer = setTimeout(() => {
            this.timer = null;
            this.waiting = false;
            this.pump();
          }, this.retryMs);
        } else {
          job.waiters.forEach((w) => w.reject(e));
        }
        this.preempted = false;
        this.changed();
        this.pump();
      },
    );
  }
}
