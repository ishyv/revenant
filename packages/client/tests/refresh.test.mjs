import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApp } from '../src/app.ts';
import { NativeHost, hasCode, manifest, turn, until, within } from './fixtures/native-host.mjs';

async function indexed(t, options = {}) {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  t.after(async () => {
    host.unblockAll();
    await within(app.disposeAsync(), 'refresh teardown');
    assert.deepEqual(app.cleanupErrors.current, []);
    for (const owners of [host.scopes, host.folders, host.views, host.resources, host.selections, host.tasks])
      assert.equal(owners.size, 0);
  });
  const folder = await app.files.openFolder();
  const files = folder.files(options);
  await within(files.ready, 'initial collection');
  await until(() => !host.views.get(files.id).outstanding, 'initial ACK');
  return { app, host, folder, files };
}

test('refresh replaces one inventory and resets viewport and selections while retaining options', async t => {
  const { host, folder, files } = await indexed(t, { recursive: true, search: '.txt', sort: 'name', descending: true });
  const other = folder.files();
  await other.loaded;
  await files.window(1, 1);
  await until(() => files.items.length === 1, 'requested viewport');
  const selection = files.selection();
  await selection.set([files.items[0]]);
  const oldGeneration = files.snapshot.generation;
  host.names = ['delta.txt', 'gamma.txt', 'picture.png'];
  await within(files.refresh(), 'complete refresh');
  assert.equal(files.snapshot.offset, 0);
  assert.equal(files.snapshot.generation, oldGeneration + 1);
  assert.equal(selection.items.length, 0);
  assert.deepEqual(files.items.map(item => item.name), ['gamma.txt']);
  assert.deepEqual(host.requests('view.refresh')[0].request.options,
    { recursive: true, search: '.txt', sort: 'name', descending: true });
  assert.deepEqual(other.items.map(item => item.name), ['alpha.txt', 'beta.txt', 'gamma.txt']);
});

test('refresh waits for scan completion and query changes stay on its inventory', async t => {
  const { host, files } = await indexed(t);
  host.refreshState = 'loading';
  host.names = ['new-a.txt', 'new-b.txt'];
  let settled = false;
  const refreshed = files.refresh().then(() => { settled = true; });
  await files.ready;
  await turn();
  assert.equal(settled, false);
  host.names = ['wrong-old-cache.txt'];
  await files.filter('new');
  await files.sort('name', true);
  assert.deepEqual(files.items.map(item => item.name), ['new-b.txt', 'new-a.txt']);
  host.scan(files.id, ['new-a.txt', 'new-b.txt', 'new-c.txt'], 'ready');
  await within(refreshed, 'latest query completes refresh');
  assert.deepEqual(files.items.map(item => item.name), ['new-c.txt', 'new-b.txt', 'new-a.txt']);
});

test('terminal Channel page before refresh reply settles without another publication', async t => {
  const { host, files } = await indexed(t);
  host.refreshState = 'loading';
  const gate = host.blockNext('view.refresh', 'reply');
  const refreshed = files.refresh();
  const admitted = await gate.entered.promise;
  // ACK admission's loading page, then complete the scan while invoke is gated.
  admitted.updates(admitted.reply.snapshot);
  await until(() => !host.views.get(admitted.reply.view).outstanding, 'loading ACK');
  host.scan(admitted.reply.view, ['completed.txt'], 'ready');
  await until(() => files.snapshot.state === 'ready', 'terminal Channel accepted');
  await until(() => !host.views.get(admitted.reply.view).outstanding, 'terminal ACK');
  gate.release.resolve();
  await within(refreshed, 'refresh completion from already accepted terminal page');
  assert.deepEqual(files.items.map(item => item.name), ['completed.txt']);
});

test('failed Channel page before admission rejects refresh without another publication', async t => {
  const { host, files } = await indexed(t);
  host.refreshState = 'loading';
  const gate = host.blockNext('view.refresh', 'reply');
  const refreshed = files.refresh();
  const rejected = assert.rejects(refreshed, hasCode('refresh_failed'));
  const admitted = await gate.entered.promise;
  admitted.updates(admitted.reply.snapshot);
  await until(() => !host.views.get(admitted.reply.view).outstanding, 'loading ACK');
  host.scan(admitted.reply.view, [], 'failed');
  await until(() => files.snapshot.state === 'failed', 'failed Channel accepted');
  gate.release.resolve();
  await within(rejected, 'failed refresh reconciled');
});

test('query replacement ignores terminal refresh page received before delayed admission', async t => {
  const { host, files } = await indexed(t);
  host.refreshState = 'loading';
  const gate = host.blockNext('view.refresh', 'reply');
  const refreshed = files.refresh();
  let settled = false;
  void refreshed.then(() => { settled = true; });
  const admitted = await gate.entered.promise;
  const query = files.filter('new');
  admitted.updates({ ...admitted.reply.snapshot, state: 'ready', revision: admitted.reply.snapshot.revision + 1 });
  await turn();
  gate.release.resolve();
  await within(query, 'query replacement');
  await turn();
  assert.equal(settled, false, 'old generation must not settle the refresh');
  host.scan(files.id, ['new.txt'], 'ready');
  await within(refreshed, 'current query completes refresh');
  assert.deepEqual(files.items.map(item => item.name), ['new.txt']);
});

test('filtering during a delayed refresh reply cannot resurrect the original inventory', async t => {
  const { host, files } = await indexed(t);
  const old = files.id;
  host.names = ['fresh.txt'];
  const gate = host.blockNext('view.refresh', 'reply');
  const refreshed = files.refresh();
  const admitted = await gate.entered.promise;
  const query = files.filter('fresh');
  host.scan(old, ['stale.txt'], 'ready');
  gate.release.resolve();
  await within(Promise.all([query, refreshed]), 'refresh and filtering');
  assert.deepEqual(files.items.map(item => item.name), ['fresh.txt']);
  assert.notEqual(files.id, old);
  assert.ok(host.requests('view.ack').some(call => call.request.view === admitted.reply.view));
});

test('overlapping refresh rejects the retired scan and admits only the latest inventory', async t => {
  const { host, files } = await indexed(t);
  host.refreshState = 'loading';
  const first = files.refresh();
  await files.ready;
  const firstView = files.id;
  const rejected = assert.rejects(first, hasCode('refresh_superseded'));
  host.refreshState = 'ready'; host.names = ['latest.txt'];
  const second = files.refresh();
  await within(Promise.all([rejected, second]), 'overlapping refresh');
  assert.deepEqual(files.items.map(item => item.name), ['latest.txt']);
  await until(() => !host.views.has(firstView), 'retired scan ownership released');
  assert.ok(host.requests('view.cancel').some(call => call.request.view === firstView));
});

test('late Channel pages from a fully retired inventory are discarded without poisoning ACK state', async t => {
  const { host, files } = await indexed(t);
  const initial = host.requests('folder.query')[0];
  const oldView = files.id;
  const oldSnapshot = structuredClone(files.snapshot);
  host.names = ['fresh.txt'];
  await files.refresh();
  await until(() => !host.views.has(oldView), 'old view fully retired');
  initial.updates({ ...oldSnapshot, revision: oldSnapshot.revision + 100 });
  await turn(); await turn();
  assert.equal(files.snapshot.state, 'ready');
  assert.deepEqual(files.items.map(item => item.name), ['fresh.txt']);
  await files.window(0, 128);
});

test('abort before and during native admission rejects without leaking late views', async t => {
  const { host, files } = await indexed(t);
  const aborted = new AbortController(); aborted.abort();
  await assert.rejects(files.refresh({ signal: aborted.signal }), hasCode('refresh_cancelled'));
  assert.equal(host.requests('view.refresh').length, 0);
  const old = files.id;
  const gate = host.blockNext('view.refresh', 'reply');
  const controller = new AbortController();
  const refreshed = files.refresh({ signal: controller.signal });
  const admitted = await gate.entered.promise;
  controller.abort();
  await assert.rejects(refreshed, hasCode('refresh_cancelled'));
  gate.release.resolve();
  await files.ready;
  await until(() => !host.views.has(admitted.reply.view), 'cancelled late view disposed');
  assert.equal(files.id, old);
  await files.refresh();
});

test('disposal rejects pending refresh while already admitted batch inputs survive replacement', async t => {
  const { app, host, files } = await indexed(t);
  const selection = files.selection();
  await selection.set([files.items[0]]);
  const operation = app.bindOperation(manifest.operations[1]);
  const batch = selection.run(operation);
  await batch.ready;
  const captured = host.tasks.get(batch.snapshot.id).inputs;
  host.refreshState = 'loading'; host.names = ['replacement.txt'];
  const refreshed = files.refresh();
  await files.ready;
  const rejected = assert.rejects(refreshed, hasCode('view_disposed'));
  await files.dispose();
  await rejected;
  assert.deepEqual(host.tasks.get(batch.snapshot.id).inputs, captured);
  assert.equal(captured[0].name, 'alpha.txt');
});

test('refresh keeps large inventory metadata in the host and its viewport bounded', async t => {
  const { host, files } = await indexed(t);
  host.names = Array.from({ length: 20_000 }, (_, index) => `entry-${String(index).padStart(6, '0')}.txt`);
  await within(files.refresh(), 'large refresh');
  assert.equal(files.snapshot.total, 20_000);
  assert.equal(files.items.length, 128);
  await files.window(19_500, 512);
  await until(() => files.items.length === 500, 'bounded final viewport');
  assert.ok(host.maxPageOwners <= 1024);
});
