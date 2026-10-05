import { test } from "node:test";
import assert from "node:assert/strict";
import { get } from "svelte/store";
import { imagePreview } from "../src/lib/models/image-preview.ts";

test("a late preview cannot replace the preview after the selection changes", async () => {
  const pending = new Map();
  const signals = new Map();
  const render = (id, large, signal) => {
    assert.equal(large, true);
    signals.set(id, signal);
    return new Promise((resolve) => pending.set(id, resolve));
  };
  const preview = imagePreview();
  const first = preview.select("first", render);
  const second = preview.select("second", render);
  assert.equal(signals.get("first").aborted, true);
  pending.get("second")("second-url");
  await second;
  pending.get("first")("stale-url");
  await first;
  assert.deepEqual(get(preview), { url: "second-url", loading: false, error: "" });
  preview.dispose();
});

test("clearing or disposing the inspector suppresses late preview errors and results", async () => {
  let reject;
  const preview = imagePreview();
  const loading = preview.select("first", () => new Promise((_resolve, fail) => { reject = fail; }));
  preview.clear();
  reject(new Error("late error"));
  await loading;
  assert.deepEqual(get(preview), { url: "", loading: false, error: "" });
  let resolve;
  const next = preview.select("second", () => new Promise((done) => { resolve = done; }));
  preview.dispose();
  const snapshot = get(preview);
  resolve("late-url");
  await next;
  assert.equal(get(preview), snapshot);
});
