import { test } from "node:test";
import assert from "node:assert/strict";
import { get } from "svelte/store";
import { noteEditor } from "../src/lib/models/note-editor.ts";
import { noteCloseGuard } from "../src/lib/models/note-close.ts";

const tick = () => new Promise((resolve) => setImmediate(resolve));
const asset = { id: "image", rootId: "root", name: "image", relativePath: "image", size: "1", note: "" };

test("reverting a draft before the save microtask leaves a clean closable editor", async () => {
  let writes = 0;
  const editor = noteEditor(async () => { writes++; throw new Error("unexpected write"); });
  editor.select(asset); editor.edit("draft");
  const saving = editor.save();
  editor.edit("");
  assert.equal(await saving, true);
  assert.equal(writes, 0);
  assert.equal(get(editor).status, "saved");
  const close = noteCloseGuard(editor, async () => "save");
  assert.equal(await close.decide(), true);
  assert.equal(close.canClose(), true);
});

test("Save close drains edits made while writing and rechecks edits before closing", async () => {
  const writes = [];
  const editor = noteEditor((input) => new Promise((resolve) => writes.push({ input, resolve })));
  editor.select(asset); editor.edit("first");
  const close = noteCloseGuard(editor, async () => "save");
  const decision = close.decide();
  await tick();
  editor.edit("second");
  writes[0].resolve({ id: asset.id, note: "first" });
  await tick();
  assert.equal(writes[1].input.note, "second");
  writes[1].resolve({ id: asset.id, note: "second" });
  assert.equal(await decision, true);
  assert.equal(close.canClose(), true);
  editor.edit("late");
  assert.equal(close.canClose(), false);
  assert.equal(get(editor).draft, "late");
});

test("Cancel and failed saves preserve the draft, while Discard admits only the captured draft", async () => {
  for (const decision of ["cancel", "save", "discard"]) {
    const editor = noteEditor(async () => { throw new Error("disk full"); });
    editor.select(asset); editor.edit("draft");
    const close = noteCloseGuard(editor, async () => decision);
    assert.equal(await close.decide(), decision === "discard");
    assert.equal(get(editor).draft, "draft");
    assert.equal(get(editor).dirty, true);
    assert.equal(close.canClose(), decision === "discard");
    editor.edit("new draft");
    assert.equal(close.canClose(), false);
  }
});
