import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApp } from '../src/app.ts';
import { callableOperation } from '../src/operation.ts';
import { sameContract, validateManifest } from '../src/schema.ts';
import { TauriTransport } from '../src/transport.ts';
import { NativeHost, manifest, hasCode, until } from './fixtures/native-host.mjs';

test('callable operations preserve task and scope ownership', async t => {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  t.after(() => app.disposeAsync());
  const api = callableOperation(app.bindOperation(manifest.operations[0]));
  const called = api({ value: 'hello' });
  await until(() => host.tasks.size === 1, 'call admitted');
  host.complete([...host.tasks.keys()][0], { result: 'hello' });
  assert.equal(await called, 'hello');
  assert.equal(host.tasks.size, 0, 'direct calls release the task after returning');
  const controlled = api.run({ value: 'controlled' });
  await until(() => host.tasks.size === 1, 'controlled call admitted');
  host.complete([...host.tasks.keys()][0], { result: 'controlled' });
  assert.equal(await controlled.result, 'controlled');
  await controlled.dispose();
  assert.equal(typeof api.query, 'function');
  app.dispose();
  assert.throws(() => api.run({ value: 'retired' }), hasCode('scope_disposed'));
});

test('native transport correlates invocations and development reports with bounded UI context', async t => {
  const originalWindow = globalThis.window;
  const originalDocument = globalThis.document;
  const listeners = new Map();
  const calls = [];
  globalThis.document = { title: 'Development fixture', visibilityState: 'visible' };
  globalThis.window = { location: { pathname: '/catalog', search: '?token=private' },
    addEventListener: (name, handler) => listeners.set(name, handler),
    removeEventListener: name => listeners.delete(name),
    __TAURI_INTERNALS__: { invoke: async (_command, { request }) => {
      calls.push(request);
      return request.action === 'root.connect' ? { scope: 'root', development: true, manifest } : {};
    } },
  };
  t.after(() => { globalThis.window = originalWindow; globalThis.document = originalDocument; });
  const transport = new TauriTransport();
  const app = createApp({ transport, manifest });
  await app.ready;
  await transport.dispatch({ action: 'task.run', scope: 'root', operation: 'titles.normalize', input: { text: 'hello' } });
  const started = calls.find(call => call.action === 'task.run');
  assert.equal(typeof started.requestId, 'string');
  assert.equal(new Set(calls.map(call => call.requestId)).size, calls.length);
  listeners.get('error')({ message: 'x'.repeat(50_000), filename: 'fixture.ts', lineno: 12, colno: 4 });
  await new Promise(resolve => setImmediate(resolve));
  const reports = calls.filter(call => call.action === 'frontend.snapshot');
  assert.ok(reports.length >= 2);
  assert.equal(reports[0].snapshot.route, '/catalog');
  assert.ok(JSON.stringify(reports).length < 16_384);
  assert.ok(!JSON.stringify(reports).includes('private'));
  await app.disposeAsync();
  assert.equal(listeners.size, 0);
});

test('query helpers require declared reads and execution metadata is part of compatibility', () => {
  const definition = manifest.operations[0];
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  const unknown = { ...definition, effect: 'unknown' };
  assert.throws(() => app.bindOperation(unknown).query(), hasCode('query_not_read_only'));
  const write = callableOperation(app.bindOperation({ ...definition, effect: 'write' }));
  assert.equal(write.query, undefined);
  for (const change of [{ effect: 'write' }, { independent: true }, { agent: true }, { arguments: [] }]) {
    assert.equal(sameContract(definition, { ...definition, ...change }), false);
  }
  for (const change of [{ effect: 'magic' }, { independent: 'yes' }, { agent: null }, { arguments: ['x', 'x'] }]) {
    assert.throws(() => validateManifest({ ...manifest, operations: [{ ...definition, ...change }] }), hasCode('invalid_contract'));
  }
  app.dispose();
});
