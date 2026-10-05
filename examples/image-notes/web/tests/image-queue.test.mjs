import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
const source = readFileSync(
  new URL("../src/lib/image-queue.ts", import.meta.url),
  "utf8",
);
const { imageQueue } = await import(
  `data:text/javascript;base64,${Buffer.from(stripTypeScriptTypes(source, { mode: "transform" })).toString("base64")}`
);
const tick = () => new Promise((resolve) => setImmediate(resolve));

test("image queue bounds admission, prioritizes previews and drops unmounted tiles", async () => {
  const started = [];
  const pending = new Map();
  const render = imageQueue((id) => {
    started.push(id);
    return new Promise((resolve) => pending.set(id, resolve));
  }, 1);
  const first = render("first", false);
  const controller = new AbortController();
  const cancelled = render("offscreen", false, controller.signal);
  const rejected = assert.rejects(cancelled, { name: "AbortError" });
  const next = render("next", false);
  const preview = render("preview", true);
  assert.deepEqual(started, ["first"]);
  controller.abort();
  await rejected;
  pending.get("first")("one");
  await first;
  await tick();
  assert.deepEqual(started, ["first", "preview"]);
  pending.get("preview")("large");
  await preview;
  await tick();
  assert.deepEqual(started, ["first", "preview", "next"]);
  pending.get("next")("two");
  await next;
  assert.equal(await render("next", false), "two");
});

test("failed images may be retried and old cached URLs are evicted", async () => {
  const calls = [];
  let fail = true;
  const render = imageQueue(
    async (id) => {
      calls.push(id);
      if (id === "broken" && fail) {
        fail = false;
        throw new Error("offline");
      }
      return id;
    },
    2,
    2,
  );
  await assert.rejects(render("broken", false));
  assert.equal(await render("broken", false), "broken");
  await render("a", false);
  await render("b", false);
  await render("broken", false);
  assert.equal(calls.filter((id) => id === "broken").length, 3);
});
