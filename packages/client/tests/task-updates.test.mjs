import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApp } from '../src/app.ts';
import { sameContract, validateManifest } from '../src/schema.ts';
import { NativeHost, manifest, hasCode, until } from './fixtures/native-host.mjs';

const definition = {
  ...manifest.operations[0],
  updates: { name: 'ScanUpdate', typescript: 'export type ScanUpdate = { count: number };', schema: {
    type: 'object', properties: { count: { type: 'integer', minimum: 0 } }, required: ['count'], additionalProperties: false,
  } },
};
const contract = { ...manifest, operations: [definition] };
function setup(t) {
  const host = new NativeHost();
  const transport = { dispatch: async (request, updates) => {
    const reply = await host.dispatch(request, updates);
    return request.action === 'root.connect' ? { ...reply, manifest: structuredClone(contract) } : reply;
  } };
  const app = createApp({ transport, manifest: contract });
  t.after(() => app.disposeAsync());
  return { host, app };
}
function emit(host, task, revision, count, overrides = {}) {
  const row = host.tasks.get(task.snapshot.id);
  row.updates({ ...structuredClone(row.snapshot), state: 'running', revision: String(revision),
    update: { revision: String(revision), value: { count } }, ...overrides });
}

test('task updates are typed latest snapshots; stale channel and refresh replies do not regress', async t => {
  const { host, app } = setup(t);
  const task = app.bindOperation(definition).run({ value: 'done' });
  await task.ready;
  const values = [];
  const unsubscribe = task.updates.subscribe(value => values.push(value));
  emit(host, task, 2, 2);
  emit(host, task, 4, 4);
  emit(host, task, 3, 3);
  await task.refresh(); // fixture host's older running snapshot
  assert.deepEqual(values, [undefined, { count: 2 }, { count: 4 }]);
  const late = [];
  task.updates.subscribe(value => late.push(value));
  assert.deepEqual(late, [{ count: 4 }]);
  const row = host.tasks.get(task.snapshot.id);
  row.updates({ ...row.snapshot, revision: '5', state: 'succeeded', result: 'done',
    update: { revision: '4', value: { count: 4 } } });
  assert.equal(await task.result, 'done');
  emit(host, task, 6, 6);
  assert.deepEqual(values, [undefined, { count: 2 }, { count: 4 }]);
  unsubscribe();
  const update = row.updates;
  await task.dispose();
  update({ ...row.snapshot, revision: '7', update: { revision: '7', value: { count: 7 } } });
  assert.deepEqual(late, [{ count: 4 }]);
});

test('invalid update schema or byte bound fails the task with a structured diagnostic', async t => {
  const { host, app } = setup(t);
  for (const value of [{ count: 'wrong' }, { count: 1, extra: 'x'.repeat(65_536) }]) {
    const task = app.bindOperation(definition).run({ value: 'done' });
    await task.ready;
    const row = host.tasks.get(task.snapshot.id);
    row.updates({ ...row.snapshot, revision: '1', update: { revision: '1', value } });
    await assert.rejects(task.result, hasCode(value.extra ? 'task_update_limit' : 'contract_violation'));
    await task.dispose();
  }
});

test('compiled update contract participates in manifest validation and host compatibility', () => {
  assert.equal(sameContract(definition, manifest.operations[0]), false);
  assert.throws(() => validateManifest({ ...contract, operations: [{ ...definition, updates: {} }] }), hasCode('invalid_contract'));
  assert.throws(() => validateManifest({ ...contract, version: 5, protocol: 4 }), hasCode('incompatible_contract'));
});

test('synchronous update subscribers cannot regress newer snapshots or publish after disposal', async t => {
  const { host, app } = setup(t);
  const task = app.bindOperation(definition).run({ value: 'done' });
  await task.ready;
  task.updates.subscribe(value => { if (value?.count === 2) emit(host, task, 4, 4); });
  emit(host, task, 2, 2);
  assert.equal(task.snapshot.revision, '4');
  assert.deepEqual(task.snapshot.update.value, { count: 4 });
  const before = structuredClone(task.snapshot);
  task.updates.subscribe(value => { if (value?.count === 6) void task.dispose(); });
  emit(host, task, 6, 6);
  const disposed = structuredClone(task.snapshot);
  emit(host, task, 8, 8);
  assert.deepEqual(task.snapshot, disposed);
  await task.dispose();
});
