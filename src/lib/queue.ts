/**
 * Runs at most `n` tasks at once; the rest wait in order. Used for clip thumbnails, so a
 * board of 30 cards doesn't start 30 FFmpeg processes together.
 */
export function limiter(n: number) {
  let running = 0;
  const waiting: (() => void)[] = [];
  const next = () => {
    if (running >= n) return;
    const start = waiting.shift();
    if (start) {
      running++;
      start();
    }
  };
  return function run<T>(task: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      waiting.push(() => {
        task()
          .then(resolve, reject)
          .finally(() => {
            running--;
            next();
          });
      });
      next();
    });
  };
}
