import { test } from "node:test";
import assert from "node:assert/strict";
import { get, writable } from "svelte/store";
import { scanModel } from "../src/lib/models/scan.ts";

test("scan progress and warning toast follow the task and release terminal ownership", async () => {
  const progress = writable({ state: "queued", progress: {} });
  let resolve;
  let cancelled = false;
  let disposed = false;
  const task = {
    subscribe: progress.subscribe,
    result: new Promise((done) => { resolve = done; }),
    async cancel() { cancelled = true; },
    async dispose() { disposed = true; },
  };
  const scan = scanModel({ run: () => task });
  const running = scan.run("folder");
  assert.equal(get(scan).scanning, true);
  progress.set({ state: "running", progress: { message: "Reading" } });
  assert.equal(get(scan).message, "Reading");
  await scan.cancel();
  assert.equal(cancelled, true);
  const report = { rootId: "root", discovered: 3, warnings: 1 };
  resolve(report);
  assert.deepEqual(await running, report);
  assert.equal(disposed, true);
  assert.equal(get(scan).scanning, false);
  assert.equal(get(scan).toast, "3 imágenes · 1 archivos no se pudieron leer");
  await scan.dispose();
});

test("disposing a scan suppresses late results and progress", async () => {
  const progress = writable({ state: "running", progress: {} });
  let resolve;
  const task = {
    subscribe: progress.subscribe,
    result: new Promise((done) => { resolve = done; }),
    async cancel() {},
    async dispose() {},
  };
  const scan = scanModel({ run: () => task });
  const running = scan.run("folder");
  await scan.dispose();
  const snapshot = get(scan);
  resolve({ rootId: "root", discovered: 3, warnings: 0 });
  progress.set({ state: "running", progress: { message: "late" } });
  assert.equal(await running, undefined);
  assert.equal(get(scan), snapshot);
});
