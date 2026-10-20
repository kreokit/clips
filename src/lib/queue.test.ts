import { describe, expect, it } from "vitest";
import { limiter } from "./queue";

describe("limiter", () => {
  it("runs at most n tasks at once, in order, and passes results and errors through", async () => {
    const run = limiter(2);
    let active = 0;
    let most = 0;
    const order: number[] = [];
    const task = (i: number) => async () => {
      active++;
      most = Math.max(most, active);
      order.push(i);
      await new Promise((r) => setTimeout(r, 5));
      active--;
      if (i === 3) throw new Error("three");
      return i * 10;
    };
    const results = await Promise.allSettled([1, 2, 3, 4, 5].map((i) => run(task(i))));
    expect(most).toBe(2);
    expect(order).toEqual([1, 2, 3, 4, 5]);
    expect(results.map((r) => (r.status === "fulfilled" ? r.value : "error"))).toEqual([10, 20, "error", 40, 50]);
  });
});
