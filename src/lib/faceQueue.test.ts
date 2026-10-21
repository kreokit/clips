import { describe, expect, it, vi } from "vitest";
import { Dropped, FaceQueue } from "./faceQueue";

/** A fake backend: each analyze_faces call waits until the test settles it. */
function backend() {
  const calls: { cmd: string; args?: Record<string, unknown>; settle?: (error?: string) => void }[] = [];
  const invoke = (cmd: string, args?: Record<string, unknown>) => {
    if (cmd !== "analyze_faces") {
      calls.push({ cmd, args });
      return Promise.resolve();
    }
    return new Promise<void>((resolve, reject) => {
      calls.push({ cmd, args, settle: (error) => (error ? reject(error) : resolve()) });
    });
  };
  const analyses = () => calls.filter((c) => c.cmd === "analyze_faces");
  const last = () => analyses()[analyses().length - 1];
  return { invoke, calls, analyses, last };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("FaceQueue", () => {
  it("runs one job at a time, in order", async () => {
    const b = backend();
    const q = new FaceQueue(b.invoke);
    const done: string[] = [];
    void q.request("p", [[0, 10]], "background").then(() => done.push("a"));
    void q.request("p", [[20, 30]], "background").then(() => done.push("b"));
    expect(b.analyses()).toHaveLength(1);
    b.last().settle!();
    await tick();
    expect(done).toEqual(["a"]);
    expect(b.analyses()).toHaveLength(2);
    expect(b.last().args).toEqual({ id: "p", ranges: [[20, 30]] });
    b.last().settle!();
    await tick();
    expect(done).toEqual(["a", "b"]);
  });

  it("lets the editor's clip go first, then resumes the stopped job", async () => {
    const b = backend();
    const q = new FaceQueue(b.invoke);
    const done: string[] = [];
    void q.request("p", [[0, 10]], "background").then(() => done.push("background"));
    void q.request("p", [[50, 60]], "now").then(() => done.push("now"));
    expect(b.calls.map((c) => c.cmd)).toEqual(["analyze_faces", "cancel_faces"]);
    b.analyses()[0].settle!("cancelled");
    await tick();
    expect(b.last().args).toEqual({ id: "p", ranges: [[50, 60]] });
    b.last().settle!();
    await tick();
    expect(b.last().args).toEqual({ id: "p", ranges: [[0, 10]] });
    b.last().settle!();
    await tick();
    expect(done).toEqual(["now", "background"]);
  });

  it("shares a job asked for twice, and promotes it", async () => {
    const b = backend();
    const q = new FaceQueue(b.invoke);
    void q.request("p", [[0, 10]], "background");
    const other = q.request("p", [[20, 30]], "background");
    const again = q.request("p", [[20, 30]], "now");
    expect(b.calls.filter((c) => c.cmd === "cancel_faces")).toHaveLength(1);
    b.analyses()[0].settle!("cancelled");
    await tick();
    expect(b.last().args).toEqual({ id: "p", ranges: [[20, 30]] });
    b.last().settle!();
    await expect(Promise.all([other, again])).resolves.toBeDefined();
  });

  it("waits while a transcription runs", async () => {
    vi.useFakeTimers();
    const b = backend();
    const q = new FaceQueue(b.invoke, 5000);
    const statuses: boolean[] = [];
    q.subscribe((s) => statuses.push(s.waitingForTranscription));
    const done = q.request("p", [[0, 10]], "now");
    b.last().settle!("transcribing");
    await vi.advanceTimersByTimeAsync(0);
    expect(q.status().waitingForTranscription).toBe(true);
    expect(b.analyses()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(5000);
    expect(b.analyses()).toHaveLength(2);
    b.last().settle!();
    await expect(done).resolves.toBeUndefined();
    expect(statuses).toContain(true);
    vi.useRealTimers();
  });

  it("retries at once when told the transcription finished", async () => {
    const b = backend();
    const q = new FaceQueue(b.invoke, 60_000);
    void q.request("p", [[0, 10]], "now");
    b.last().settle!("transcribing");
    await tick();
    q.retrySoon();
    expect(b.analyses()).toHaveLength(2);
  });

  it("drops a project's jobs and stops the running one", async () => {
    const b = backend();
    const q = new FaceQueue(b.invoke);
    const running = q.request("p", [[0, 10]], "background");
    const waiting = q.request("p", [[20, 30]], "background");
    const other = q.request("q", [[0, 10]], "background");
    q.clear("p");
    await expect(running).rejects.toBeInstanceOf(Dropped);
    await expect(waiting).rejects.toBeInstanceOf(Dropped);
    expect(b.calls.some((c) => c.cmd === "cancel_faces")).toBe(true);
    b.analyses()[0].settle!("cancelled");
    await tick();
    expect(b.last().args).toEqual({ id: "q", ranges: [[0, 10]] });
    b.last().settle!();
    await expect(other).resolves.toBeUndefined();
  });

  it("passes other errors on", async () => {
    const b = backend();
    const q = new FaceQueue(b.invoke);
    const job = q.request("p", [[0, 10]], "now");
    b.last().settle!("ffmpeg could not read the video");
    await expect(job).rejects.toBe("ffmpeg could not read the video");
  });
});
