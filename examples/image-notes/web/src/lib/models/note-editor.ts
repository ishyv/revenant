import { writable } from "svelte/store";
import { message, type Asset, type ImageOperations, type SavedNote } from "../types";

export interface NoteSnapshot {
  asset?: Asset;
  draft: string;
  dirty: boolean;
  status: "saved" | "dirty" | "saving" | "failed";
  error: string;
}

/**
 * Own one editable annotation. A shared save drains every edit made during an
 * in-flight write before resolving, so navigation never discards a newer draft.
 * Writes are serialized; the committed note changes without replacing the draft.
 */
export function noteEditor(
  annotate: Pick<ImageOperations["annotate"], "call">,
  onSaved: (saved: SavedNote) => void = () => {},
) {
  let state: NoteSnapshot = {
    draft: "",
    dirty: false,
    status: "saved",
    error: "",
  };
  const store = writable(state);
  let alive = true;
  let selection = 0;
  let work: Promise<boolean> | undefined;

  function publish(next: NoteSnapshot) {
    state = next;
    store.set(state);
  }

  async function drain(): Promise<boolean> {
    while (alive && state.asset && state.dirty) {
      const asset = state.asset;
      const note = state.draft;
      const version = selection;
      publish({ ...state, status: "saving", error: "" });
      try {
        const saved = await annotate.call({ id: asset.id, note });
        if (!alive) return false;
        if (version === selection) {
          const dirty = state.draft !== saved.note;
          publish({
            ...state,
            asset: { ...asset, note: saved.note },
            dirty,
            status: dirty ? "dirty" : "saved",
          });
        }
        onSaved(saved);
      } catch (failure) {
        if (alive && version === selection) {
          publish({ ...state, status: "failed", error: message(failure) });
        }
        return false;
      }
    }
    return alive;
  }

  return {
    subscribe: store.subscribe,
    /** Call only after the coordinator has flushed the previous annotation. */
    select(asset?: Asset) {
      if (!alive) return;
      selection++;
      publish({
        asset,
        draft: asset?.note ?? "",
        dirty: false,
        status: "saved",
        error: "",
      });
    },
    edit(draft: string) {
      if (!alive || !state.asset) return;
      const dirty = draft !== state.asset.note;
      publish({
        ...state,
        draft,
        dirty,
        status: work ? "saving" : dirty ? "dirty" : "saved",
      });
    },
    save(): Promise<boolean> {
      if (!alive) return Promise.resolve(false);
      if (work) return work;
      // Install the shared promise before publishing; store subscribers can reenter.
      work = Promise.resolve()
        .then(drain)
        .finally(() => {
          work = undefined;
        });
      return work;
    },
    dismissError() {
      if (alive) publish({ ...state, error: "" });
    },
    dispose() {
      alive = false;
      selection++;
    },
  };
}

export type NoteEditor = ReturnType<typeof noteEditor>;
