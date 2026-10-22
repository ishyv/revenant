import assert from 'node:assert/strict';
import { test } from 'node:test';
import { queryGroup } from '../src/query-group.ts';
import { createApp } from '../src/app.ts';
import { callableOperation } from '../src/operation.ts';
import { NativeHost, manifest, until, hasCode, nativeError } from './fixtures/native-host.mjs';

function fixture(t) {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  t.after(() => app.disposeAsync());
  const read = callableOperation(app.bindOperation(manifest.operations[0]), true);
  const group = queryGroup(app.scope, { required: { title: read }, optional: { extra: read } });
  return { host, app, read, group };
}

test('group reads preserve exact native handle ownership and never dispose the dataset owner', async t => {
  class StrictHandleHost extends NativeHost {
    admit(request, updates) {
      if (request.action === 'task.run') {
        const handle = request.input.handle;
        const owned = this.resources.get(handle.id)?.handle;
        if (!owned || owned.scope !== request.scope || handle.scope !== owned.scope)
          throw nativeError('scope_mismatch', 'Dataset belongs to another native scope');
      }
      return super.admit(request, updates);
    }
  }
  const host = new StrictHandleHost();
  const app = createApp({ transport: host, manifest });
  t.after(() => app.disposeAsync());
  await app.ready;
  const scope = app.scope.id;
  const handle = { id: 'dataset', scope, generation: 1, kind: 'dataset' };
  host.resources.set(handle.id, { handle });
  const read = callableOperation(app.bindOperation(manifest.operations[1]), true);
  const group = queryGroup(app.scope, { required: { summary: read } });
  const loading = group.load('dataset', { summary: { handle, name: 'native dataset' } });
  await until(() => host.requests('task.run').length === 1, 'native dataset admission');
  for (const id of [...host.tasks.keys()]) host.complete(id, { result: '500000' });
  await loading;
  assert.equal(group.snapshot.status, 'ready');
  assert.deepEqual(group.snapshot.data, { summary: '500000' });
  assert.equal(host.requests('task.run')[0].request.scope, scope);
  await group.dispose();
  assert.equal(host.tasks.size, 0);
  assert.equal(host.scopes.has(scope), true);
  assert.equal(host.resources.get(handle.id).handle, handle);
  assert.equal(host.requests('scope.create').length, 0);
  assert.equal(host.requests('scope.dispose').length, 0);
});

test('group publishes settled slots atomically, tags data and omits failed optional current data', async t => {
  const { host, group } = fixture(t);
  const states = [];
  group.subscribe(state => states.push(state));
  const loading = group.load('a', { title: { value: 'title' }, extra: { value: 'extra' } });
  await until(() => host.tasks.size === 2, 'group slots');
  const [first, second] = [...host.tasks.keys()];
  host.complete(first, { result: 'a-title' });
  assert.equal(group.snapshot.data, undefined);
  host.complete(second, { state: 'failed' });
  await loading;
  assert.equal(group.snapshot.status, 'ready');
  assert.equal(group.snapshot.dataKey, 'a');
  assert.deepEqual(group.snapshot.data, { title: 'a-title' });
  assert.equal(group.snapshot.errors.extra.code, 'failed');
  const failed = group.load('b', { title: { value: 'next' }, extra: { value: 'next' } });
  await until(() => host.tasks.size === 2, 'next group slots');
  const [next, optional] = [...host.tasks.keys()];
  host.complete(optional, { result: 'b-extra' });
  host.complete(next, { state: 'failed' });
  await failed;
  assert.equal(group.snapshot.status, 'failed');
  assert.equal(group.snapshot.requestKey, 'b');
  assert.equal(group.snapshot.dataKey, 'a');
  assert.deepEqual(group.snapshot.data, { title: 'a-title' });
  assert.ok(states.every(s => !s.data || 'title' in s.data));
});

test('storm admits one active and only the latest pending group, releasing superseded work', async t => {
  const { host, group } = fixture(t);
  const gate = host.blockNext('task.run', 'reply');
  const initial = group.load('0', { title: { value: '0' }, extra: { value: '0' } });
  await gate.entered.promise;
  const promises = Array.from({ length: 100 }, (_, i) => group.load(String(i + 1), {
    title: { value: String(i + 1) }, extra: { value: String(i + 1) },
  }));
  assert.equal(host.requests('task.run').length, 2);
  gate.release.resolve();
  await until(() => host.requests('task.run').length === 4 && host.tasks.size === 2, 'latest pending slots');
  for (const [id, task] of host.tasks) {
    assert.equal(task.inputs[0].value, '100');
    host.complete(id, { result: 'latest' });
  }
  await Promise.all([initial, ...promises]);
  assert.equal(group.snapshot.dataKey, '100');
  assert.equal(host.tasks.size, 0);
});

test('replacement waits for query task disposal acknowledgement without closing the owner scope', async t => {
  const { host, app, group } = fixture(t);
  const initial = group.load('old', { title: { value: 'old' }, extra: { value: 'old' } });
  await until(() => host.tasks.size === 2, 'initial owned reads');
  const gate = host.blockNext('task.dispose', 'reply');
  const replaced = group.load('discarded', { title: { value: 'discarded' }, extra: { value: 'discarded' } });
  await gate.entered.promise;
  const latest = group.load('latest', { title: { value: 'latest' }, extra: { value: 'latest' } });
  await replaced;
  assert.equal(host.requests('task.run').length, 2);
  assert.equal(host.scopes.has(app.scope.id), true);
  assert.equal(host.requests('scope.dispose').length, 0);
  gate.release.resolve();
  await until(() => host.requests('task.run').length === 4 && host.tasks.size === 2, 'latest reads after cleanup');
  for (const [id, task] of host.tasks) {
    assert.equal(task.snapshot.scope, app.scope.id);
    assert.equal(task.inputs[0].value, 'latest');
    host.complete(id, { result: 'latest' });
  }
  await Promise.all([initial, latest]);
  assert.equal(group.snapshot.dataKey, 'latest');
  assert.equal(host.tasks.size, 0);
});

test('root closure suppresses publication and waits for late admission cleanup', async t => {
  const { host, app, group } = fixture(t);
  const gate = host.blockNext('task.run', 'reply');
  const loading = group.load('a', { title: { value: 'a' }, extra: { value: 'a' } });
  await gate.entered.promise;
  const closing = app.disposeAsync();
  gate.release.resolve();
  await Promise.all([loading, closing]);
  assert.equal(group.snapshot.data, undefined);
  assert.equal(host.tasks.size, 0);
  assert.throws(() => group.load('b', {}), hasCode('scope_disposed'));
});

test('group rejects writes, foreign operations, duplicate names and excess slots', async t => {
  const { app, read } = fixture(t);
  const other = createApp({ transport: new NativeHost(), manifest });
  t.after(() => other.disposeAsync());
  const foreign = callableOperation(other.bindOperation(manifest.operations[0]), true);
  assert.throws(() => queryGroup(app.scope, { required: { foreign } }), hasCode('scope_mismatch'));
  const write = callableOperation(app.bindOperation({ ...manifest.operations[0], effect: 'write' }), false);
  assert.throws(() => queryGroup(app.scope, { required: { write } }), hasCode('query_not_read_only'));
  assert.throws(() => queryGroup(app.scope, { required: { read }, optional: { read } }), hasCode('query_group_slots'));
  assert.throws(() => queryGroup(app.scope, { required: Object.fromEntries(Array.from({ length: 17 }, (_, i) => [i, read])) }), hasCode('query_group_slots'));
});

test('a load immediately after settlement is admitted and reentrant supersession keeps only latest data', async t => {
  const { host, group } = fixture(t);
  let replace = true;
  let replacement;
  group.subscribe(state => {
    if (state.status === 'ready' && replace) {
      replace = false;
      replacement = group.load('b', { title: { value: 'b' }, extra: { value: 'b' } });
    }
  });
  const first = group.load('a', { title: { value: 'a' }, extra: { value: 'a' } });
  await until(() => host.tasks.size === 2, 'first tasks');
  for (const id of [...host.tasks.keys()]) host.complete(id, { result: 'a' });
  await first;
  await until(() => host.tasks.size === 2, 'replacement tasks');
  for (const id of [...host.tasks.keys()]) host.complete(id, { result: 'b' });
  await replacement;
  const next = group.load('c', { title: { value: 'c' }, extra: { value: 'c' } });
  await until(() => host.tasks.size === 2, 'post-settlement tasks');
  for (const id of [...host.tasks.keys()]) host.complete(id, { result: 'c' });
  await next;
  assert.equal(group.snapshot.dataKey, 'c');
});

test('group bounds encoded pending input independently of slot count', async t => {
  const { group } = fixture(t);
  assert.throws(() => group.load('large', { title: { value: 'a'.repeat(1024 * 1024) }, extra: { value: '' } }), hasCode('query_group_input_limit'));
});

test('declared slot names remain own data keys without invoking prototype setters', async t => {
  const { app, host, read } = fixture(t);
  const group = queryGroup(app.scope, { required: Object.fromEntries([['__proto__', read]]) });
  const loading = group.load('key', Object.fromEntries([['__proto__', { value: 'safe' }]]));
  await until(() => host.tasks.size === 1, 'prototype-named slot');
  host.complete([...host.tasks.keys()][0], { result: 'safe' });
  await loading;
  assert.equal(Object.getPrototypeOf(group.snapshot.data), Object.prototype);
  assert.equal(Object.hasOwn(group.snapshot.data, '__proto__'), true);
  assert.equal(group.snapshot.data.__proto__, 'safe');
});

test('skipped optional reads admit required work immediately and omit prior optional data', async t => {
  const { host, group } = fixture(t);
  const first = group.load('first', { title: { value: 'first' }, extra: { value: 'first' } });
  await until(() => host.tasks.size === 2, 'initial slots');
  for (const id of [...host.tasks.keys()]) host.complete(id, { result: 'first' });
  await first;
  assert.equal(group.snapshot.data.extra, 'first');
  const next = group.load('next', { title: { value: 'next' }, extra: { value: 'unused' } }, { skipOptional: ['extra'] });
  await until(() => host.tasks.size > 0, 'required slot');
  assert.equal(host.tasks.size, 1);
  host.complete([...host.tasks.keys()][0], { result: 'next' });
  await next;
  assert.equal(group.snapshot.dataKey, 'next');
  assert.deepEqual(group.snapshot.data, { title: 'next' });
  assert.deepEqual(group.snapshot.errors, {});
  assert.equal(host.requests('task.run').length, 3);
  assert.equal(host.tasks.size, 0);
});

test('optional skip policy rejects required and unknown names before changing a live load', async t => {
  const { host, group } = fixture(t);
  const first = group.load('first', { title: { value: 'first' }, extra: { value: 'first' } });
  await until(() => host.tasks.size === 2, 'live slots');
  for (const skipOptional of [['title'], ['unknown'], ['extra', 'extra'], 'extra', null, Array(17).fill('extra')])
    assert.throws(() => group.load('bad', { title: { value: 'bad' }, extra: { value: 'bad' } }, { skipOptional }), hasCode('query_group_optional'));
  assert.equal(group.snapshot.requestKey, 'first');
  assert.equal(host.requests('task.run').length, 2);
  for (const id of [...host.tasks.keys()]) host.complete(id, { result: 'first' });
  await first;
  assert.equal(group.snapshot.dataKey, 'first');
  assert.throws(() => group.load('missing', { title: { value: 'missing' } }, { skipOptional: ['extra'] }), hasCode('query_group_inputs'));
});

test('skip selection is captured with pending inputs and required failures retain the preceding key', async t => {
  const { host, group } = fixture(t);
  const selection = ['extra'];
  const first = group.load('first', { title: { value: 'first' }, extra: { value: 'unused' } }, { skipOptional: selection });
  selection.length = 0;
  await until(() => host.tasks.size > 0, 'captured selection');
  assert.equal(host.tasks.size, 1);
  host.complete([...host.tasks.keys()][0], { result: 'first' });
  await first;
  const failed = group.load('failed', { title: { value: 'failed' }, extra: { value: 'unused' } }, { skipOptional: ['extra'] });
  await until(() => host.tasks.size > 0, 'failed required slot');
  host.complete([...host.tasks.keys()][0], { state: 'failed' });
  await failed;
  assert.equal(group.snapshot.status, 'failed');
  assert.equal(group.snapshot.requestKey, 'failed');
  assert.equal(group.snapshot.dataKey, 'first');
  assert.deepEqual(group.snapshot.data, { title: 'first' });
  assert.ok(group.snapshot.errors.title);
  assert.equal(Object.hasOwn(group.snapshot.errors, 'extra'), false);
});

test('newest pending skip selection settles with actual task disposal on root close', async t => {
  const { host, app, group } = fixture(t);
  const gate = host.blockNext('task.run', 'reply');
  const first = group.load('first', { title: { value: 'first' }, extra: { value: 'unused' } }, { skipOptional: ['extra'] });
  await gate.entered.promise;
  const second = group.load('second', { title: { value: 'second' }, extra: { value: 'second' } });
  const latest = group.load('latest', { title: { value: 'latest' }, extra: { value: 'unused' } }, { skipOptional: ['extra'] });
  gate.release.resolve();
  await until(() => host.requests('task.run').some(item => item.request.input.value === 'latest'), 'latest required admission');
  assert.equal(host.requests('task.run').filter(item => item.request.input.value === 'unused').length, 0);
  const closing = app.disposeAsync();
  await Promise.all([first, second, latest, closing]);
  assert.equal(group.snapshot.data, undefined);
  assert.equal(host.tasks.size, 0);
  assert.equal(host.scopes.size, 0);
});
