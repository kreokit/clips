/**
 * Debounced autosave (no Save dialog, CLAUDE.md "Projects"): `schedule` coalesces changes
 * and saves the latest value after `delayMs` of quiet; `flush` saves now. Saves never
 * overlap: a change during a save is saved right after it, so the newest value always
 * reaches the disk last.
 */
export interface Autosave<T> {
  schedule(value: T): void;
  /** Saves any pending value now; resolves when everything is on disk. */
  flush(): Promise<void>;
  /** Drops a pending value (e.g. the project was removed). */
  cancel(): void;
  readonly pending: boolean;
}

export function createAutosave<T>(
  save: (value: T) => Promise<unknown>,
  delayMs: number,
  onError: (error: unknown) => void = () => {},
): Autosave<T> {
  let next: { value: T } | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let running: Promise<void> | null = null;

  const clearTimer = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };

  const run = (): Promise<void> => {
    clearTimer();
    if (running) return running;
    if (!next) return Promise.resolve();
    running = (async () => {
      while (next) {
        const { value } = next;
        next = null;
        try {
          await save(value);
        } catch (e) {
          onError(e);
        }
      }
      running = null;
    })();
    return running;
  };

  return {
    schedule(value) {
      next = { value };
      clearTimer();
      if (!running) timer = setTimeout(() => void run(), delayMs);
    },
    async flush() {
      // A save in progress picks up `next` when it finishes.
      await run();
      if (next) await run();
    },
    cancel() {
      next = null;
      clearTimer();
    },
    get pending() {
      return next !== null || running !== null;
    },
  };
}
