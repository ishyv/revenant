import { writable } from "svelte/store";
import { message, type ImageOperations, type ScanReport } from "../types";

export interface ScanSnapshot {
  scanning: boolean;
  message: string;
  toast: string;
  error: string;
}

/** Own one native scan subscription/task, including terminal result cleanup. */
export function scanModel(operation: Pick<ImageOperations["scan"], "run">) {
  let state: ScanSnapshot = {
    scanning: false,
    message: "",
    toast: "",
    error: "",
  };
  const store = writable(state);
  let alive = true;
  let task: ReturnType<ImageOperations["scan"]["run"]> | undefined;
  let stop: (() => void) | undefined;
  let work: Promise<ScanReport | undefined> | undefined;

  function publish(patch: Partial<ScanSnapshot>) {
    if (!alive) return;
    state = { ...state, ...patch };
    store.set(state);
  }

  async function consume(current: NonNullable<typeof task>) {
    try {
      const result = await current.result;
      if (!alive) return;
      publish({
        toast: result.warnings
          ? `${result.discovered} imágenes · ${result.warnings} archivos no se pudieron leer`
          : `${result.discovered} imágenes listas`,
      });
      return result;
    } catch (failure) {
      publish({ error: message(failure) });
      // The orchestrator refreshes the catalog even after a partial/failed scan.
      throw failure;
    } finally {
      stop?.();
      stop = undefined;
      await current.dispose();
      if (task === current) task = undefined;
      work = undefined;
      publish({ scanning: false });
    }
  }

  return {
    subscribe: store.subscribe,
    run(path: string): Promise<ScanReport | undefined> {
      if (!alive) return Promise.resolve(undefined);
      if (work) return work;
      publish({
        scanning: true,
        message: "",
        error: "",
        toast: "",
      });
      try {
        task = operation.run({ path });
        stop = task.subscribe((snapshot) => {
          publish({ message: snapshot.progress.message ?? "" });
        });
        work = consume(task);
        return work;
      } catch (failure) {
        publish({ scanning: false, error: message(failure) });
        return Promise.reject(failure);
      }
    },
    async cancel() {
      try {
        await task?.cancel();
      } catch (failure) {
        publish({ error: message(failure) });
      }
    },
    dismissToast() {
      publish({ toast: "" });
    },
    dismissError() {
      publish({ error: "" });
    },
    async dispose() {
      if (!alive) return;
      alive = false;
      stop?.();
      stop = undefined;
      await task?.dispose();
    },
  };
}
