import type { ImageRenderer } from "./types";

/** Bound decode admission and retained frontend data URLs independently of catalog size. */
export function imageQueue(
  render: ImageRenderer,
  concurrency = 2,
  maxCached = 80,
): ImageRenderer {
  let active = 0;
  const waiting: Array<{ start: () => void; cancel: () => void }> = [];
  const cache = new Map<string, Promise<string>>();
  const evict = () => {
    while (cache.size > maxCached) cache.delete(cache.keys().next().value!);
  };
  return (id, large, signal) => {
    if (signal?.aborted)
      return Promise.reject(new DOMException("Cancelled", "AbortError"));
    const key = `${id}:${large}`;
    const existing = cache.get(key);
    if (existing) {
      cache.delete(key);
      cache.set(key, existing);
      return existing;
    }
    const result = new Promise<string>((resolve, reject) => {
      let started = false;
      const cancel = () => {
        if (started) return;
        const index = waiting.findIndex((work) => work.start === start);
        if (index >= 0) waiting.splice(index, 1);
        reject(new DOMException("Cancelled", "AbortError"));
      };
      const start = () => {
        signal?.removeEventListener("abort", cancel);
        started = true;
        active++;
        render(id, large)
          .then(resolve, reject)
          .finally(() => {
            active--;
            waiting.shift()?.start();
          });
      };
      if (active < concurrency) start();
      else {
        const work = { start, cancel };
        if (large) waiting.unshift(work);
        else waiting.push(work);
        signal?.addEventListener("abort", cancel, { once: true });
      }
    });
    cache.set(key, result);
    evict();
    void result.catch(() => {
      if (cache.get(key) === result) cache.delete(key);
    });
    return result;
  };
}
