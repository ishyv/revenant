import type { ImageOperations, ImageRenderer } from "./types";

/** Admit one-shot native reads through the queue; Operation.call owns task cleanup. */
export function createImageRenderer(
  operation: Pick<ImageOperations["render"], "call">,
): ImageRenderer {
  return imageQueue(async (id, large) => {
    const rendition = await operation.call({ id, large });
    return rendition.dataUrl;
  });
}

/**
 * Bound decode admission and retained data URLs independently of catalog size.
 * Abort removes waiting work only; an admitted decode finishes and its consumer
 * must suppress late publication. Eviction drops cache ownership, not active work.
 */
export function imageQueue(
  render: ImageRenderer,
  concurrency = 2,
  maxCached = 80,
): ImageRenderer {
  let active = 0;
  const waiting: Array<{ start: () => void; cancel: () => void }> = [];
  const cache = new Map<string, Promise<string>>();
  const evict = () => {
    while (cache.size > maxCached) {
      cache.delete(cache.keys().next().value!);
    }
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
        if (large) {
          waiting.unshift(work);
        } else {
          waiting.push(work);
        }
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
