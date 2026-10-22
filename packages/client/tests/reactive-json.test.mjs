import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApp } from '../src/app.ts';
import { cloneJson } from '../src/contracts.ts';
import { proxy as svelteState } from 'svelte/internal/client';
import { NativeHost, manifest, hasCode, until } from './fixtures/native-host.mjs';

function reactive(value) {
  return value && typeof value === 'object' ? new Proxy(value, {
    get(target, key, receiver) { return reactive(Reflect.get(target, key, receiver)); },
  }) : value;
}

test('reactive resource handle inputs and batches are captured before later edits', async t => {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  t.after(() => app.disposeAsync());
  await app.ready;
  const input = { name: 'original', handle: { id: 'session', scope: app.scope.id, generation: 1, kind: 'native:Session' } };
  app.operation('files.describe').run(reactive(input));
  app.operation('files.describe').runBatch(reactive([input]));
  input.name = 'later'; input.handle.id = 'changed';
  await until(() => host.requests('task.run').length && host.requests('task.batch').length, 'reactive admission');
  for (const value of [host.requests('task.run')[0].request.input, host.requests('task.batch')[0].request.selection.inputs[0]]) {
    assert.equal(value.name, 'original'); assert.equal(value.handle.id, 'session');
    assert.doesNotThrow(() => structuredClone(value));
  }
  assert.throws(() => app.operation('records.echo').run(reactive({ value: 2 })), hasCode('contract_violation'));
  assert.throws(() => app.operation('files.describe').run(reactive({ ...input, size: Number.MAX_SAFE_INTEGER + 1 })), hasCode('invalid_json'));
});

test('reactive settings defaults and edits persist detached immutable values', async t => {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  t.after(() => app.disposeAsync());
  const settings = app.settings.define(reactive({ theme: 'dark', panels: [10, 20] }), { key: 'reactive' });
  await settings.ready;
  const edit = { theme: 'light', panels: [30, 40] };
  settings.set(reactive(edit));
  edit.panels[0] = 999;
  await settings.flush();
  assert.deepEqual(settings.value, { theme: 'light', panels: [30, 40] });
  assert.deepEqual(host.settings.get('reactive'), settings.value);
});

test('Svelte state copies preserve JSON restrictions and data keys without prototype setters', () => {
  const source = svelteState({ rows: [{ size: '9007199254740993' }] });
  const captured = cloneJson(source);
  source.rows[0].size = '2';
  assert.equal(captured.rows[0].size, '9007199254740993');
  for (const value of [undefined, NaN, Infinity, 9007199254740992, new Date(), { callback() {} }])
    assert.throws(() => cloneJson(value), hasCode('invalid_json'));
  const cycle = {}; cycle.self = cycle;
  assert.throws(() => cloneJson(svelteState(cycle)), hasCode('invalid_json'));
  const ownKey = cloneJson(JSON.parse('{"__proto__":{"polluted":true}}'));
  assert.equal(Object.getPrototypeOf(ownKey), Object.prototype);
  assert.equal(Object.hasOwn(ownKey, '__proto__'), true);
  assert.equal({}.polluted, undefined);
});

test('reactive runtime configuration remains typed and detached before admission', async t => {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  t.after(() => app.disposeAsync());
  const [provider] = await app.runtime.inspect();
  const input = { enabled: false };
  const change = app.runtime.configure(provider, svelteState(input));
  input.enabled = true;
  await change;
  assert.deepEqual(host.requests('runtime.configure')[0].request.config, { enabled: false });
  await assert.rejects(app.runtime.configure(provider, reactive({ enabled: 'invalid' })), hasCode('contract_violation'));
});
