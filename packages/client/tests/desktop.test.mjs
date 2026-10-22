import assert from "node:assert/strict";
import { test } from "node:test";
import { askSaveChanges, onDesktopCloseRequested } from "../src/desktop.ts";

const tick = () => new Promise((resolve) => setImmediate(resolve));
function target() {
  let handler;
  let destroyed = 0;
  let removed = 0;
  return {
    window: {
      async onCloseRequested(next) { handler = next; return () => { removed++; }; },
      async destroy() { destroyed++; },
    },
    async close() { let prevented = false; await handler({ preventDefault() { prevented = true; } }); assert.equal(prevented, true); },
    get destroyed() { return destroyed; },
    get removed() { return removed; },
  };
}

test("native close requests share one decision and final admission rechecks edits", async () => {
  const host = target();
  let resolve;
  let decisions = 0;
  let clean = true;
  const stop = await onDesktopCloseRequested(() => {
    decisions++; return new Promise((done) => { resolve = done; });
  }, { window: host.window, canClose: () => clean });
  const first = host.close();
  await tick();
  await host.close();
  assert.equal(decisions, 1);
  clean = false;
  resolve(true);
  await first;
  assert.equal(host.destroyed, 0);
  const next = host.close(); await tick(); clean = true; resolve(true); await next;
  assert.equal(host.destroyed, 1);
  stop(); assert.equal(host.removed, 1);
});

test("close cancellation, failure and teardown retain the window", async () => {
  for (const decide of [() => false, () => { throw new Error("save failed"); }]) {
    const host = target();
    const failures = [];
    const stop = await onDesktopCloseRequested(decide, { window: host.window, onError: (error) => failures.push(error) });
    await host.close(); assert.equal(host.destroyed, 0); stop();
  }
  const host = target();
  let resolve;
  const stop = await onDesktopCloseRequested(() => new Promise((done) => { resolve = done; }), { window: host.window });
  const pending = host.close(); await tick(); stop(); resolve(true); await pending;
  assert.equal(host.destroyed, 0);
});

test("native save dialog uses explicit labels and treats dismissal as Cancel", async (t) => {
  const original = globalThis.window;
  t.after(() => { globalThis.window = original; });
  let response = "Guardar";
  globalThis.window = { __TAURI_INTERNALS__: { async invoke(command, options) {
    assert.equal(command, "plugin:dialog|message");
    assert.deepEqual(options.buttons, { YesNoCancelCustom: ["Guardar", "Descartar", "Cancelar"] });
    return response;
  } } };
  const labels = { save: "Guardar", discard: "Descartar", cancel: "Cancelar" };
  assert.equal(await askSaveChanges("Close?", labels), "save");
  response = "Descartar"; assert.equal(await askSaveChanges("Close?", labels), "discard");
  for (response of ["Cancelar", "Cancel", "", "unexpected"]) {
    assert.equal(await askSaveChanges("Close?", labels), "cancel");
  }
});
