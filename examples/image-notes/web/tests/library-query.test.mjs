import { test } from "node:test";
import assert from "node:assert/strict";
import { get, writable } from "svelte/store";
import { libraryQuery } from "../src/lib/models/library-query.ts";

function query(read) {
  const store = writable({ status: "idle" });
  let version = 0;
  return {
    subscribe: store.subscribe,
    invalidations: 0,
    disposed: false,
    async load(input) {
      const current = ++version;
      store.set({ ...get(store), status: "loading", error: undefined });
      try {
        const data = await read(input);
        if (current === version) store.set({ status: "ready", data });
      } catch (error) {
        if (current === version) store.set({ ...get(store), status: "failed", error });
      }
    },
    invalidate() {
      this.invalidations++;
      version++;
      store.set({ ...get(store), status: "idle" });
    },
    async dispose() {
      this.disposed = true;
      version++;
    },
  };
}

test("library queries keep bounded pages and reset an offset beyond the new total", async () => {
  const calls = [];
  const bootstrap = query(async () => ({ roots: [], dataPath: "local" }));
  const page = query(async (input) => {
    calls.push(input);
    return { items: [], total: 1, offset: input.offset };
  });
  const library = libraryQuery({ bootstrap: { query: () => bootstrap }, query: { query: () => page } });
  await library.start();
  assert.equal(get(library).ready, true);
  await library.navigate(1);
  assert.deepEqual(calls.map((input) => input.offset), [0, 48, 0]);
  assert.ok(calls.every((input) => input.limit === 48));
  assert.equal(get(library).offset, 0);
  await library.dispose();
  assert.equal(bootstrap.disposed, true);
  assert.equal(page.disposed, true);
});

test("invalidating a library read suppresses a late page before a debounced search", async () => {
  let resolve;
  const bootstrap = query(async () => ({ roots: [], dataPath: "local" }));
  const page = query(() => new Promise((done) => { resolve = done; }));
  const library = libraryQuery({ bootstrap: { query: () => bootstrap }, query: { query: () => page } });
  const loading = library.start();
  await new Promise((done) => setImmediate(done));
  library.invalidate();
  resolve({ items: [{ id: "stale" }], total: 1, offset: 0 });
  await loading;
  assert.deepEqual(get(library).page.items, []);
  await library.dispose();
});

test("query failures are readable and dismissible without throwing from load", async () => {
  const bootstrap = query(async () => { throw { code: "offline", message: "native unavailable" }; });
  const page = query(async () => assert.fail("page cannot load before bootstrap"));
  const library = libraryQuery({ bootstrap: { query: () => bootstrap }, query: { query: () => page } });
  await library.start();
  assert.equal(get(library).ready, false);
  assert.equal(get(library).error, "native unavailable");
  library.dismissError();
  assert.equal(get(library).error, "");
  await library.dispose();
});

test("retained query data never overwrites a locally committed note", async () => {
  let fail = false;
  const bootstrap = query(async () => ({ roots: [], dataPath: "local" }));
  const page = query(async () => {
    if (fail) throw { code: "offline", message: "refresh failed" };
    return { items: [{ id: "image", note: "old" }], total: 1, offset: 0 };
  });
  const library = libraryQuery({ bootstrap: { query: () => bootstrap }, query: { query: () => page } });
  await library.start();
  library.noteSaved({ id: "image", note: "committed" });
  library.invalidate();
  assert.equal(get(library).page.items[0].note, "committed");
  fail = true;
  const refresh = library.reload();
  assert.equal(get(library).loading, true);
  assert.equal(get(library).page.items[0].note, "committed");
  await refresh;
  assert.equal(get(library).page.items[0].note, "committed");
  assert.equal(get(library).error, "refresh failed");
  await library.dispose();
});

test("refresh keeps the current page and applies a pending search at the first page", async () => {
  const calls = [];
  const bootstrap = query(async () => ({ roots: [], dataPath: "local" }));
  const page = query(async (input) => {
    calls.push(input);
    return { items: [], total: 100, offset: input.offset };
  });
  const library = libraryQuery({ bootstrap: { query: () => bootstrap }, query: { query: () => page } });
  await library.start();
  await library.navigate(1);
  await library.reload();
  assert.equal(calls.at(-1).offset, 48);
  await library.reload("new search");
  assert.equal(calls.at(-1).search, "new search");
  assert.equal(calls.at(-1).offset, 0);
  await library.dispose();
});

test("choosing a root retains annotation filtering unless the UI explicitly resets it", async () => {
  const calls = [];
  const bootstrap = query(async () => ({ roots: [], dataPath: "local" }));
  const page = query(async (input) => {
    calls.push(input);
    return { items: [], total: 0, offset: input.offset };
  });
  const library = libraryQuery({ bootstrap: { query: () => bootstrap }, query: { query: () => page } });
  await library.start();
  await library.chooseRoot("", true);
  await library.chooseRoot("root");
  assert.equal(get(library).annotatedOnly, true);
  assert.equal(calls.at(-1).annotatedOnly, true);
  await library.chooseRoot("", false);
  assert.equal(get(library).annotatedOnly, false);
  await library.dispose();
});
