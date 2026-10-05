import { get, writable } from "svelte/store";
import { message, type AssetPage, type ImageOperations, type Root, type SavedNote } from "../types";

export const pageSize = 48;

export interface LibrarySnapshot {
  roots: Root[];
  dataPath: string;
  page: AssetPage;
  rootId: string;
  search: string;
  annotatedOnly: boolean;
  offset: number;
  ready: boolean;
  loading: boolean;
  error: string;
}

/**
 * Project two scope-owned native queries into the library UI. Only one bounded
 * page is retained; query generations/cancellation belong to the framework.
 * Input intent must invalidate immediately, even before a debounced load starts.
 */
export function libraryQuery(operations: Pick<ImageOperations, "bootstrap" | "query">) {
  const startup = operations.bootstrap.query();
  const catalog = operations.query.query();
  let alive = true;
  let state: LibrarySnapshot = {
    roots: [],
    dataPath: "",
    page: {
      items: [],
      total: 0,
      offset: 0,
    },
    rootId: "",
    search: "",
    annotatedOnly: false,
    offset: 0,
    ready: false,
    loading: false,
    error: "",
  };
  const store = writable(state);
  let lastPage: AssetPage | undefined;

  function publish(patch: Partial<LibrarySnapshot>) {
    if (!alive) return;
    state = { ...state, ...patch };
    store.set(state);
  }

  const stopStartup = startup.subscribe((snapshot) => {
    if (!alive) return;
    if (snapshot.data) {
      publish({ roots: snapshot.data.roots, dataPath: snapshot.data.dataPath, ready: true });
    }
    if (snapshot.status === "failed") {
      publish({ error: message(snapshot.error) });
    }
  });
  const stopCatalog = catalog.subscribe((snapshot) => {
    if (!alive) return;
    // Loading/invalidation retains data. Do not undo a locally committed annotation
    // by projecting that identical old page a second time.
    if (snapshot.data && snapshot.data !== lastPage) {
      lastPage = snapshot.data;
      publish({ page: snapshot.data });
    }
    publish({ loading: snapshot.status === "loading" });
    if (snapshot.status === "failed") {
      publish({ error: message(snapshot.error) });
    }
  });

  async function reload(search = state.search): Promise<void> {
    if (!alive || !state.ready) return;
    if (search !== state.search) publish({ search, offset: 0 });
    publish({ error: "" });
    await catalog.load({
      search: state.search,
      rootId: state.rootId,
      annotatedOnly: state.annotatedOnly,
      offset: state.offset,
      limit: pageSize,
    });
    if (!alive) return;
    const snapshot = get(catalog);
    if (
      snapshot.status === "ready" && snapshot.data &&
      snapshot.data.total > 0 && snapshot.data.offset >= snapshot.data.total
    ) {
      publish({ offset: 0 });
      await reload();
    }
  }

  async function refreshRoots(): Promise<void> {
    if (!alive) return;
    publish({ error: "" });
    await startup.load({});
  }

  return {
    subscribe: store.subscribe,
    async start() {
      await refreshRoots();
      await reload();
    },
    refreshRoots,
    reload,
    invalidate() {
      if (alive) catalog.invalidate();
    },
    async search(value: string) {
      if (!alive) return;
      catalog.invalidate();
      publish({ search: value, offset: 0 });
      await reload();
    },
    async chooseRoot(rootId: string, annotatedOnly = state.annotatedOnly, search = state.search) {
      if (!alive) return;
      catalog.invalidate();
      publish({ rootId, annotatedOnly, search, offset: 0 });
      await reload();
    },
    async navigate(delta: number, search = state.search) {
      if (!alive) return;
      catalog.invalidate();
      const offset = search === state.search ? state.offset : 0;
      publish({ search, offset: Math.max(0, offset + delta * pageSize) });
      await reload();
    },
    noteSaved(saved: SavedNote) {
      if (!alive) return;
      // Any read admitted before this commit may still contain the old note.
      catalog.invalidate();
      publish({
        page: {
          ...state.page,
          items: state.page.items.map((asset) =>
            asset.id === saved.id ? { ...asset, note: saved.note } : asset,
          ),
        },
      });
    },
    dismissError() {
      publish({ error: "" });
    },
    async dispose() {
      if (!alive) return;
      alive = false;
      stopStartup();
      stopCatalog();
      await Promise.all([startup.dispose(), catalog.dispose()]);
    },
  };
}
