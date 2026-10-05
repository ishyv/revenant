import { writable } from "svelte/store";
import { message, type ImageRenderer } from "../types";

/** Inspector-owned preview lifetime; queued work is cancelled when selection changes. */
export function imagePreview() {
  const store = writable({
    url: "",
    loading: false,
    error: "",
  });
  let version = 0;
  let alive = true;
  let controller: AbortController | undefined;

  function invalidate() {
    version++;
    controller?.abort();
    controller = undefined;
  }

  return {
    subscribe: store.subscribe,
    async select(id: string, render: ImageRenderer): Promise<void> {
      if (!alive) return;
      invalidate();
      const current = version;
      controller = new AbortController();
      store.set({ url: "", loading: true, error: "" });
      try {
        const url = await render(id, true, controller.signal);
        // Started native decodes cannot be cancelled by the queue; suppress late results.
        if (alive && current === version) {
          store.set({ url, loading: false, error: "" });
        }
      } catch (failure) {
        if (alive && current === version) {
          store.set({ url: "", loading: false, error: message(failure) });
        }
      }
    },
    clear() {
      invalidate();
      if (alive) store.set({ url: "", loading: false, error: "" });
    },
    dispose() {
      alive = false;
      invalidate();
    },
  };
}
