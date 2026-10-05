import { test } from "node:test";
import assert from "node:assert/strict";
import { get } from "svelte/store";
import { noteEditor } from "../src/lib/models/note-editor.ts";
import { coordinator } from "../src/lib/models/coordinator.ts";

const asset = (id) => ({
  id, rootId: "root", name: id, relativePath: id, size: "1024", note: "",
});
const tick = () => new Promise((resolve) => setImmediate(resolve));

test("destructive actions wait for asynchronous confirmation and respect cancellation", async () => {
  let resolve;
  let saved = 0;
  let removed = 0;
  const navigation = coordinator({ save: async () => { saved++; return true; } });
  const moving = navigation.run(() => { removed++; }, () => new Promise((done) => { resolve = done; }));
  await tick();
  assert.equal(saved, 0);
  assert.equal(removed, 0);
  resolve(false);
  assert.equal(await moving, false);
  assert.equal(removed, 0);
  assert.equal(await navigation.run(() => { removed++; }, async () => true), true);
  assert.equal(saved, 1);
  assert.equal(removed, 1);
  navigation.dispose();
});

test("failed or late confirmations never run destructive actions", async () => {
  let removed = false;
  const navigation = coordinator({ save: async () => true });
  await assert.rejects(navigation.run(() => { removed = true; }, async () => {
    throw new Error("confirmation unavailable");
  }), /confirmation unavailable/);
  let resolve;
  const moving = navigation.run(() => { removed = true; }, () => new Promise((done) => { resolve = done; }));
  await tick();
  navigation.dispose();
  resolve(true);
  assert.equal(await moving, false);
  assert.equal(removed, false);
});

test("navigation waits for edits made while the first note save is pending", async () => {
  const pending = [];
  const committed = [];
  const editor = noteEditor({
    call(input) {
      return new Promise((resolve) => pending.push({ input, resolve }));
    },
  }, (saved) => committed.push(saved));
  const navigation = coordinator(editor);
  editor.select(asset("first"));
  editor.edit("first draft");
  const saving = editor.save();
  await tick();
  editor.edit("newer draft");
  const moving = navigation.run(() => editor.select(asset("second")));
  pending[0].resolve(pending[0].input);
  await tick();
  assert.equal(get(editor).asset.id, "first");
  assert.equal(get(editor).draft, "newer draft");
  assert.equal(get(editor).dirty, true);
  assert.equal(pending.length, 2);
  assert.deepEqual(pending[1].input, { id: "first", note: "newer draft" });
  pending[1].resolve(pending[1].input);
  assert.equal(await saving, true);
  assert.equal(await moving, true);
  assert.equal(get(editor).asset.id, "second");
  assert.deepEqual(committed.map((value) => value.note), ["first draft", "newer draft"]);
  navigation.dispose();
  editor.dispose();
});

test("failed saves block navigation and refresh and retain the draft for retry", async () => {
  let fail = true;
  const editor = noteEditor({
    async call(input) {
      if (fail) throw new Error("disk full");
      return input;
    },
  });
  const navigation = coordinator(editor);
  editor.select(asset("first"));
  editor.edit("keep this");
  let refreshed = false;
  assert.equal(await navigation.run(() => {
    refreshed = true;
    editor.select(asset("second"));
  }), false);
  assert.equal(refreshed, false);
  assert.equal(get(editor).asset.id, "first");
  assert.equal(get(editor).draft, "keep this");
  assert.equal(get(editor).status, "failed");
  assert.equal(get(editor).error, "disk full");
  fail = false;
  assert.equal(await navigation.run(() => { refreshed = true; }), true);
  assert.equal(refreshed, true);
  assert.equal(get(editor).dirty, false);
  navigation.dispose();
  editor.dispose();
});

test("disposing a note editor suppresses a late save and queued navigation", async () => {
  let resolve;
  let updates = 0;
  const editor = noteEditor({ call: () => new Promise((done) => { resolve = done; }) }, () => updates++);
  const navigation = coordinator(editor);
  editor.select(asset("first"));
  editor.edit("draft");
  let moved = false;
  const moving = navigation.run(() => { moved = true; });
  await tick();
  navigation.dispose();
  editor.dispose();
  const snapshot = get(editor);
  resolve({ id: "first", note: "draft" });
  assert.equal(await moving, false);
  assert.equal(moved, false);
  assert.equal(updates, 0);
  assert.equal(get(editor), snapshot);
});
