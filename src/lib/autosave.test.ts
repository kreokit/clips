import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createAutosave } from "./autosave";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

/** A save that records values and resolves when `release` is called. */
function slowSave() {
  const saved: number[] = [];
  const waiting: (() => void)[] = [];
  const save = vi.fn(
    (v: number) =>
      new Promise<void>((resolve) => {
        waiting.push(() => {
          saved.push(v);
          resolve();
        });
      }),
  );
  const release = async () => {
    waiting.shift()?.();
    await vi.advanceTimersByTimeAsync(0);
  };
  return { save, saved, release };
}

describe("createAutosave", () => {
  it("saves the latest value once after a quiet period", async () => {
    const save = vi.fn(async (_: number) => {});
    const auto = createAutosave(save, 500);
    auto.schedule(1);
    await vi.advanceTimersByTimeAsync(300);
    auto.schedule(2);
    await vi.advanceTimersByTimeAsync(300);
    expect(save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    expect(save.mock.calls).toEqual([[2]]);
    expect(auto.pending).toBe(false);
  });

  it("flush saves right away", async () => {
    const save = vi.fn(async (_: number) => {});
    const auto = createAutosave(save, 500);
    auto.schedule(7);
    await auto.flush();
    expect(save.mock.calls).toEqual([[7]]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("never overlaps saves and writes the newest value last", async () => {
    const { save, saved, release } = slowSave();
    const auto = createAutosave(save, 100);
    auto.schedule(1);
    await vi.advanceTimersByTimeAsync(100);
    expect(save).toHaveBeenCalledTimes(1);
    auto.schedule(2);
    auto.schedule(3);
    await vi.advanceTimersByTimeAsync(500);
    expect(save).toHaveBeenCalledTimes(1); // still waiting for the first save
    await release();
    expect(save).toHaveBeenCalledTimes(2);
    await release();
    expect(saved).toEqual([1, 3]);
    expect(auto.pending).toBe(false);
  });

  it("flush waits for a save in progress and the change after it", async () => {
    const { save, saved, release } = slowSave();
    const auto = createAutosave(save, 100);
    auto.schedule(1);
    await vi.advanceTimersByTimeAsync(100);
    auto.schedule(2);
    let done = false;
    void auto.flush().then(() => (done = true));
    await release();
    expect(done).toBe(false);
    await release();
    expect(done).toBe(true);
    expect(saved).toEqual([1, 2]);
  });

  it("reports errors and keeps working", async () => {
    const onError = vi.fn();
    const save = vi.fn(async (v: number) => {
      if (v === 1) throw new Error("disk full");
    });
    const auto = createAutosave(save, 10, onError);
    auto.schedule(1);
    await auto.flush();
    auto.schedule(2);
    await auto.flush();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(save.mock.calls).toEqual([[1], [2]]);
  });

  it("cancel drops a pending value", async () => {
    const save = vi.fn(async (_: number) => {});
    const auto = createAutosave(save, 100);
    auto.schedule(1);
    auto.cancel();
    await vi.advanceTimersByTimeAsync(500);
    await auto.flush();
    expect(save).not.toHaveBeenCalled();
  });
});
