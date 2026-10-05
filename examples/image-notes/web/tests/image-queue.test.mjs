import { test } from "node:test";
import assert from "node:assert/strict";
import { createImageRenderer, imageQueue } from "../src/lib/image-queue.ts";
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

test("native rendering admits two calls and retains at most eighty cached URLs", async () => {
  const calls = [];
  const pending = new Map();
  let hold = true;
  const render = createImageRenderer({
    call(input) {
      calls.push(input);
      if (!hold) return Promise.resolve({ dataUrl: input.id });
      return new Promise((resolve) => pending.set(input.id, resolve));
    },
  });
  const first = render("first", false);
  const second = render("second", true);
  const third = render("third", false);
  assert.deepEqual(calls, [
    { id: "first", large: false },
    { id: "second", large: true },
  ]);
  pending.get("first")({ dataUrl: "first-url" });
  assert.equal(await first, "first-url");
  await tick();
  assert.equal(calls.length, 3);
  pending.get("second")({ dataUrl: "second-url" });
  pending.get("third")({ dataUrl: "third-url" });
  await Promise.all([second, third]);
  assert.equal(await render("first", false), "first-url");
  assert.equal(calls.length, 3);
  hold = false;
  for (let index = 0; index < 80; index++) await render(`image-${index}`, false);
  assert.equal(await render("first", false), "first");
  assert.equal(calls.filter((input) => input.id === "first").length, 2);
});
