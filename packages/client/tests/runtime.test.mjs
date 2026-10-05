import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApp } from '../src/app.ts';
import { confirm } from '../src/transport.ts';
import * as svelteContext from '../src/svelte.ts';
import { compile } from 'svelte/compiler';
import { render } from 'svelte/server';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { NativeHost, hasCode, manifest, nativeError, turn, until, within } from './fixtures/native-host.mjs';

test('native confirmation accepts only Yes and waits for the dialog response', async t => {
  const original = globalThis.window;
  t.after(() => { globalThis.window = original; });
  let resolve;
  globalThis.window = { __TAURI_INTERNALS__: { invoke(command, options) {
    assert.equal(command, 'plugin:dialog|message');
    assert.deepEqual(options, { message: 'Remove folder?', buttons: 'YesNo' });
    return new Promise(done => { resolve = done; });
  } } };
  const result = confirm('Remove folder?');
  let settled = false;
  void result.then(() => { settled = true; });
  await turn();
  assert.equal(settled, false);
  resolve('Yes');
  assert.equal(await result, true);
  for (const response of ['No', 'Cancel', 'Ok', true]) {
    const cancelled = confirm('Remove folder?');
    resolve(response);
    assert.equal(await cancelled, false);
  }
});

test('native confirmation reports dialog failures instead of approving', async t => {
  const original = globalThis.window;
  t.after(() => { globalThis.window = original; });
  globalThis.window = { __TAURI_INTERNALS__: { invoke: async () => { throw new Error('dialog unavailable'); } } };
  await assert.rejects(confirm('Remove folder?'), /dialog unavailable/);
});

function application(t, host = new NativeHost(), options = {}) {
  const app = createApp({ transport: host, manifest, ...options });
  t.after(async () => {
    host.unblockAll();
    await within(app.disposeAsync(), 'application teardown');
    assert.deepEqual(app.cleanupErrors.current, [], 'Cleanup failures must remain observable and empty here');
    for (const owners of [host.scopes, host.folders, host.views, host.resources, host.selections, host.tasks, host.previews])
      assert.equal(owners.size, 0, 'All native owners must be reclaimed');
  });
  return { app, host };
}

async function indexed(t, host = new NativeHost()) {
  const { app } = application(t, host);
  const folder = await app.files.openFolder();
  const files = folder.files();
  await within(files.ready, 'initial query');
  await until(() => !host.views.get(files.id).outstanding, 'initial page ACK');
  return { app, host, folder, files };
}

async function selected(t) {
  const setup = await indexed(t);
  const selection = setup.files.selection();
  await selection.ready;
  await selection.set([setup.files.items[0]]);
  return { ...setup, selection };
}

function echoQuery(app) {
  return app.bindOperation(manifest.operations[0]).query();
}

async function admitted(host, value) {
  await until(() => [...host.tasks.values()].some(task => task.inputs[0]?.value === value), `query ${value} admitted`);
  return [...host.tasks.values()].find(task => task.inputs[0]?.value === value).snapshot.id;
}

test('operation queries discard late results and reclaim superseded native tasks', async t => {
  const { app, host } = application(t);
  const query = echoQuery(app);
  const gate = host.blockNext('task.run', 'reply');
  const old = query.load({ value: 'old' });
  const stale = await gate.entered.promise;
  const latest = query.load({ value: 'latest' });
  host.complete(await admitted(host, 'latest'), { result: 'latest result' });
  await latest;
  stale.updates({ ...stale.reply, state: 'succeeded', result: 'old result' });
  gate.release.resolve();
  await old;
  assert.deepEqual(query.snapshot, { status: 'ready', data: 'latest result' });
  assert.equal(host.tasks.size, 0);
});

test('operation queries retain data while refreshing and capture recoverable failures', async t => {
  const { app, host } = application(t);
  const query = echoQuery(app);
  const initial = query.load({ value: 'initial' });
  host.complete(await admitted(host, 'initial'), { result: 'previous' });
  await initial;
  const refresh = query.load({ value: 'refresh' });
  assert.deepEqual(query.snapshot, { status: 'loading', data: 'previous' });
  const id = await admitted(host, 'refresh');
  host.tasks.get(id).snapshot.error = { code: 'read_failed', message: 'Read failed', retryable: true };
  host.complete(id, { state: 'failed' });
  await refresh;
  assert.equal(query.snapshot.status, 'failed');
  assert.equal(query.snapshot.data, 'previous');
  assert.equal(query.snapshot.error.code, 'read_failed');
  const retry = query.load({ value: 'retry' });
  host.complete(await admitted(host, 'retry'), { result: 'recovered' });
  await retry;
  assert.deepEqual(query.snapshot, { status: 'ready', data: 'recovered' });
});

test('query invalidation suppresses old responses before a debounced request begins', async t => {
  const { app, host } = application(t);
  const query = echoQuery(app);
  const gate = host.blockNext('task.run', 'reply');
  const pending = query.load({ value: 'old search' });
  const old = await gate.entered.promise;
  query.invalidate();
  old.updates({ ...old.reply, state: 'succeeded', result: 'stale search' });
  assert.equal(query.snapshot.status, 'idle');
  assert.equal(query.snapshot.data, undefined);
  gate.release.resolve();
  await pending;
  assert.equal(host.tasks.size, 0);
});

test('scope disposal freezes query state while pending native admission is reclaimed', async t => {
  const { app, host } = application(t);
  const query = echoQuery(app);
  const publications = [];
  query.subscribe(value => publications.push(value));
  const gate = host.blockNext('task.run', 'reply');
  const pending = query.load({ value: 'pending' });
  const old = await gate.entered.promise;
  app.dispose();
  const count = publications.length;
  old.updates({ ...old.reply, state: 'succeeded', result: 'after unmount' });
  gate.release.resolve();
  await pending;
  await app.disposeAsync();
  assert.equal(publications.length, count);
  assert.equal(query.snapshot.data, undefined);
  assert.equal(host.tasks.size, 0);
  await query.dispose();
});

test('queries record invalid input without submitting a native task', async t => {
  const { app, host } = application(t);
  const query = echoQuery(app);
  await query.load({ value: 12 });
  assert.equal(query.snapshot.status, 'failed');
  assert.ok(query.snapshot.error.code);
  assert.equal(host.requests('task.run').length, 0);
});

test('query subscriptions can replace a load synchronously without admitting obsolete work', async t => {
  const { app, host } = application(t);
  const query = echoQuery(app);
  let replacement;
  const stop = query.subscribe(snapshot => {
    if (snapshot.status === 'loading' && !replacement) {
      replacement = Promise.resolve();
      replacement = query.load({ value: 'replacement' });
    }
  });
  t.after(stop);
  await query.load({ value: 'obsolete' });
  host.complete(await admitted(host, 'replacement'), { result: 'replacement' });
  await replacement;
  assert.equal(query.snapshot.data, 'replacement');
  assert.equal(host.requests('task.run').length, 1);
});

test('query cleanup failures remain observable on the owning scope', async t => {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  t.after(() => app.disposeAsync());
  const dispatch = host.dispatch.bind(host);
  host.dispatch = async (request, updates) => {
    const result = await dispatch(request, updates);
    if (request.action === 'task.dispose') throw nativeError('cleanup_failed', 'Task cleanup failed');
    return result;
  };
  const query = echoQuery(app);
  const pending = query.load({ value: 'cleanup' });
  host.complete(await admitted(host, 'cleanup'), { result: 'value' });
  await pending;
  assert.equal(query.snapshot.data, 'value');
  assert.equal(app.cleanupErrors.current.length, 1);
  assert.equal(app.cleanupErrors.current[0].code, 'cleanup_failed');
});

test('Svelte setup provides the existing root and disposes root and child on component teardown', async t => {
  assert.equal(typeof svelteContext.setupApp, 'function');
  const { app, host } = application(t);
  const directory = mkdtempSync(join(dirname(fileURLToPath(import.meta.url)), '.setup-'));
  try {
    const component = compile(`<script>
      import { setupApp, useApp } from '../../src/svelte.ts';
      let { app, observe } = $props();
      const root = setupApp(app);
      const child = useApp();
      observe(root, child);
    </script>`, { generate: 'server', filename: 'Setup.svelte' });
    const entry = join(directory, 'Setup.mjs');
    writeFileSync(entry, component.js.code);
    let scoped;
    await render((await import(pathToFileURL(entry).href)).default, { props: {
      app,
      observe(root, child) { assert.equal(root, app); scoped = child; },
    } });
    assert.equal(scoped.scope.parent, app.scope);
    await app.disposeAsync();
    assert.equal(app.scope.disposed, true);
    assert.equal(scoped.scope.disposed, true);
    assert.equal(host.scopes.size, 0);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('picker cancellation, native bookmark restoration and preview ownership', async t => {
  const { app, host } = application(t);
  host.cancelledPicker = true;
  assert.equal(await app.files.openFolder(), undefined);
  host.cancelledPicker = false;
  const folder = await app.files.reopenBookmark({ path: 'C:\\restored-fixture' });
  assert.equal(folder.path, 'C:\\restored-fixture');
  const bookmark = folder.bookmark;
  bookmark.path = 'edited presentation copy';
  assert.equal(folder.bookmark.path, folder.path);
  const files = folder.files();
  await files.ready;
  const preview = await app.media.preview(files.items[0]);
  assert.equal(preview.name, 'alpha.txt');
  assert.match(preview.url, /^http:\/\/revenant-preview\.localhost\/preview-/);
  assert.equal(preview.url, preview.descriptor.url, 'Native URL must be used verbatim');
  await preview.dispose();
  assert.equal(preview.url, undefined);
  assert.equal(host.previews.size, 0);
});

test('search replacement captures a fresh native selection before releasing the prior view selection', async t => {
  const { host, files, selection } = await selected(t);
  const before = host.requests('selection.set').at(-1).request;
  await files.filter('beta');
  await within(selection.ready, 'new query selection');
  await selection.set([files.items[0]]);
  const after = host.requests('selection.set').at(-1).request;
  assert.notEqual(after.view, before.view);
  assert.equal(after.selection, undefined, 'A previous-view selection cannot update the new view');
  assert.equal(selection.status.current.state, 'ready');
  assert.equal(selection.items[0].name, 'beta.txt');
  assert.equal(host.selections.size, 1);
});

test('early Channel page resolves selection capture before initial ACK without a ready cycle', async t => {
  const { app, host } = application(t);
  host.streamQueryBeforeReply = true;
  const gate = host.blockNext('folder.query', 'reply');
  const folder = await app.files.openFolder();
  const files = folder.files();
  const selection = files.selection();
  let capture;
  const stop = files.subscribe(snapshot => {
    if (!capture && snapshot.items.length) capture = selection.set([snapshot.items[0]]);
  });
  t.after(stop);
  const initial = await gate.entered.promise;
  await turn();
  assert.equal(files.snapshot.view, initial.reply.view);
  assert.equal(host.requests('view.ack').length, 0, 'Pending capture must protect the initial page');
  host.scan(initial.reply.view, ['new.txt'], 'ready');
  gate.release.resolve();
  await within(files.ready, 'query ready before capture barrier');
  await within(capture, 'selection admitted from streamed initial page');
  await until(() => files.items[0]?.name === 'new.txt', 'coalesced scan after initial ACK');
  assert.equal(selection.items[0].name, 'alpha.txt', 'Capture survives native replacement');
  const captured = host.events.findIndex(event => event.phase === 'captured' && event.handles.length);
  const ack = host.events.findIndex(event => event.phase === 'ack');
  assert.ok(captured < ack);
  await selection.set(selection.items); // prior captured lease remains admissible
});

test('unsolicited changed page waits for pending selection; scan updates coalesce before scrolling', async t => {
  const { host, files } = await indexed(t);
  const selection = files.selection();
  await selection.ready;
  const old = files.items[0];
  const gate = host.blockNext('selection.set');
  const capture = selection.set([old]);
  await gate.entered.promise;
  host.scan(files.id, ['new-a.txt', 'new-b.txt']);
  await until(() => files.items[0]?.name === 'new-a.txt', 'unsolicited replacement projection');
  assert.ok(host.resources.has(old.handle.id), 'Old page owner cannot be released during admission');
  assert.equal(host.views.get(files.id).revision, 2);
  assert.equal(host.events.some(event => event.phase === 'ack' && event.revision === 2), false);
  host.scan(files.id, ['intermediate.txt']);
  host.scan(files.id, ['final.txt'], 'ready');
  const viewport = files.window(0, 1);
  await turn();
  assert.equal(host.requests('view.window').length, 0, 'Scrolling must not bypass the capture/ACK sequence');
  assert.equal(host.views.get(files.id).revision, 2, 'Pending scans cannot allocate replacement pages');
  gate.release.resolve();
  await within(capture, 'native selection capture');
  await within(viewport, 'scroll after coalesced ACKs');
  assert.deepEqual(files.items.map(item => item.name), ['final.txt']);
  assert.equal(files.snapshot.indexGeneration, 4);
  assert.equal(host.views.get(files.id).revision, 4, 'Only the latest scan and requested viewport publish');
  const captured = host.events.findIndex(event => event.phase === 'captured' && event.handles[0]?.id === old.handle.id);
  const ack = host.events.findIndex(event => event.phase === 'ack' && event.revision === 2);
  assert.ok(captured < ack);
  assert.ok(host.maxPageOwners <= 6, 'Two small windows, with no growing inventory in client ownership');
  assert.equal(host.resources.has(old.handle.id), false, 'Old page owner released after capture');
  await selection.set([old, files.items[0]]);
  assert.deepEqual(selection.items.map(item => item.name), ['alpha.txt', 'final.txt']);
});

test('stale query stream is discarded but ACKed using its own view and revision', async t => {
  const { host, files } = await indexed(t);
  const oldView = files.id;
  const gate = host.blockNext('view.query', 'reply');
  const changed = files.query({ recursive: true, search: 'beta' });
  await gate.entered.promise;
  host.scan(oldView, ['stale.txt'], 'ready');
  await until(() => !host.views.get(oldView).outstanding, 'discarded old-generation page ACK');
  assert.deepEqual(files.items, []);
  assert.equal(files.snapshot.generation, 2);
  assert.ok(host.requests('view.ack').some(call => call.request.view === oldView && call.request.revision === 2));
  gate.release.resolve();
  await within(changed, 'replacement recursive query');
  await until(() => !host.views.has(oldView), 'previous view reclaimed');
  assert.notEqual(files.id, oldView);
  assert.deepEqual(files.items.map(item => item.name), ['beta.txt']);
  assert.equal(files.snapshot.revision, 1, 'Publication ordering is per-view, not global');
});

test('rapid search edits coalesce; pending all-matching submission fails stale without deadlocking', async t => {
  const { app, host, files } = await indexed(t);
  const matching = files.selection().allMatching();
  const task = matching.run(app.operation('files.describe'));
  const rejected = assert.rejects(task.result, hasCode('stale_selection'));
  const obsolete = files.filter('alpha');
  const latest = files.filter('gamma');
  await within(Promise.all([obsolete, latest, rejected]), 'query and pending batch race');
  assert.deepEqual(files.items.map(item => item.name), ['gamma.txt']);
  assert.equal(host.requests('view.query').length, 1, 'Superseded query is never admitted');
  assert.equal(host.requests('task.batch').length, 0, 'Stale selector cannot submit another query');
  const selection = files.selection();
  await selection.dispose();
  assert.throws(() => selection.allMatching(), hasCode('selection_disposed'));
});

test('all-matching selector includes generation, checked atomically at native batch admission', async t => {
  const { app, host, files } = await indexed(t);
  const gate = host.blockNext('task.batch');
  const task = files.selection().allMatching().run(app.operation('files.describe'));
  const request = (await gate.entered.promise).request;
  assert.deepEqual(request.selection, { view: files.id, allMatching: true, generation: 1 });
  host.views.get(files.id).generation = 2; // another native actor changed the query
  const rejected = assert.rejects(task.result, hasCode('stale_selection'));
  gate.release.resolve();
  await within(rejected, 'native generation check');
  assert.equal(host.tasks.size, 0);
});

test('queued scrolling discards late viewport rows, ACKs them, and coalesces intermediate requests', async t => {
  const { host, files } = await indexed(t);
  const gate = host.blockNext('view.window', 'reply');
  const first = files.window(0, 1);
  const { reply } = await gate.entered.promise;
  const intermediate = files.window(1, 1);
  const latest = files.window(2, 1);
  gate.release.resolve();
  await within(Promise.all([first, intermediate, latest]), 'latest viewport');
  assert.deepEqual(files.items.map(item => item.name), ['gamma.txt']);
  assert.deepEqual(host.requests('view.window').map(call => call.request.offset), [0, 2]);
  assert.ok(host.requests('view.ack').some(call => call.request.revision === reply.revision));
  assert.equal(host.requests('view.window').at(-1).request.view, files.id);
});

test('scope teardown waits for late view creation and reclaims its streamed and returned pages', async t => {
  const { app, host } = application(t);
  host.streamQueryBeforeReply = true;
  const gate = host.blockNext('folder.query', 'reply');
  const folder = await app.files.openFolder();
  const files = folder.files();
  files.selection(); // capture is still awaiting initial ready
  const { reply } = await gate.entered.promise;
  const closing = app.disposeAsync();
  await turn();
  assert.ok(host.views.has(reply.view));
  assert.ok(host.scopes.size > 0);
  gate.release.resolve();
  await within(closing, 'late view/scope teardown');
  assert.equal(files.snapshot.state, 'disposed');
  assert.equal(host.views.size, 0);
  const ack = host.events.findIndex(event => event.phase === 'ack' && event.view === reply.view);
  const dispose = host.events.findIndex(event => event.phase === 'request' && event.action === 'view.dispose');
  assert.ok(ack >= 0 && ack < dispose);
});

test('batch admission freezes selected inputs before a later edit; partial results remain paged', async t => {
  const { app, host, selection } = await selected(t);
  const gate = host.blockNext('task.batch', 'reply');
  const task = selection.run(app.operation('files.describe'));
  const { reply } = await gate.entered.promise;
  const captured = structuredClone(host.tasks.get(reply.id).inputs);
  const selectionRequests = host.requests('selection.set').length;
  const clear = selection.clear();
  await turn();
  assert.equal(host.requests('selection.set').length, selectionRequests, 'Edits wait until batch admission is acknowledged');
  gate.release.resolve();
  await within(Promise.all([task.ready, clear]), 'batch admission then selection edit');
  assert.equal(selection.items.length, 0);
  assert.equal(captured[0].name, 'alpha.txt');
  host.complete(reply.id, {
    state: 'partial', rows: [
      { ordinal: '9007199254740993', input: captured[0], output: 'alpha.txt' },
      { ordinal: '18446744073709551615', input: { name: 'denied.txt' }, error: { code: 'denied', message: 'Access denied', retryable: false } },
    ],
  });
  const batch = await within(task.result, 'partial result without UI subscription');
  const pages = [];
  for await (const page of batch.pages(1)) pages.push(page);
  assert.deepEqual(pages.map(page => page.items[0].ordinal), ['9007199254740993', '18446744073709551615']);
  assert.deepEqual(pages[0].counts, { completed: '2', succeeded: '1', failed: '1' });
  assert.equal(pages[1].items[0].error.code, 'denied');
  assert.equal(host.tasks.has(reply.id), true, 'Awaiting task.result must not destroy batch results');
  await task.dispose();
  await assert.rejects(batch.page(), hasCode('task_disposed'));
});

test('terminal task Channel before invoke reply settles result independently and never regresses', async t => {
  const { app, host } = application(t);
  const gate = host.blockNext('task.run', 'reply');
  const operation = app.operation('records.echo');
  const task = operation.run({ value: 'finished' });
  const { reply } = await gate.entered.promise;
  host.complete(reply.id, { result: 'finished' });
  assert.equal(await within(task.result, 'early terminal Channel result'), 'finished');
  assert.equal(task.snapshot.state, 'succeeded');
  gate.release.resolve();
  await task.ready;
  assert.equal(task.snapshot.state, 'succeeded');
  assert.equal(task.snapshot.result, 'finished');
  assert.deepEqual(operation.definition.source, manifest.operations[0].source);
});

test('compiled contracts reject invalid input before IPC and invalid successful output', async t => {
  const { app, host } = application(t);
  const operation = app.operation('records.echo');
  assert.throws(() => operation.run({ value: 12 }), hasCode('contract_violation'));
  assert.equal(host.requests('task.run').length, 0);
  const task = operation.run({ value: 'valid input' });
  await task.ready;
  host.complete(task.snapshot.id, { result: { wrong: 'type' } });
  await assert.rejects(task.result, hasCode('contract_violation'));
  assert.equal(task.snapshot.error.code, 'contract_violation');
});

test('JSON record batches enforce 512 and encoded 1 MiB bounds and freeze caller values', async t => {
  const { app, host } = application(t);
  const operation = app.operation('records.echo');
  assert.throws(() => operation.runBatch(Array.from({ length: 513 }, () => ({ value: 'x' }))), hasCode('batch_limit'));
  assert.throws(() => operation.runBatch([{ value: 'x'.repeat(1024 * 1024) }]), hasCode('batch_limit'));
  const exact = operation.runBatch([{ value: 'x'.repeat(1024 * 1024 - 14) }]);
  await exact.ready;
  assert.equal(Buffer.byteLength(JSON.stringify(host.requests('task.batch')[0].request.selection.inputs)), 1024 * 1024);
  const inputs = Array.from({ length: 512 }, (_, index) => ({ value: String(index) }));
  const task = operation.runBatch(inputs);
  inputs[0].value = 'changed after submission';
  await task.ready;
  const admitted = host.tasks.get(task.snapshot.id).inputs;
  assert.equal(admitted.length, 512);
  assert.equal(admitted[0].value, '0');
});

test('cancellation remains nonterminal until native acknowledgement and keeps completed rows', async t => {
  const { app, host } = application(t);
  const task = app.operation('records.echo').runBatch([{ value: 'first' }, { value: 'later' }]);
  await task.ready;
  await task.cancel();
  assert.equal(task.snapshot.state, 'cancelling');
  assert.equal(task.terminal, false);
  const rejected = assert.rejects(task.result, hasCode('cancelled'));
  host.complete(task.snapshot.id, { state: 'cancelled', rows: [{ ordinal: '0', input: { value: 'first' }, output: 'first' }] });
  await rejected;
  const page = await task.results();
  assert.equal(page.items[0].output, 'first');
  assert.equal(page.counts.completed, '1');
});

test('explicit selection admits exactly 512, retains leases across scrolling and rejects overflow/foreign capabilities', async t => {
  const host = new NativeHost();
  host.names = Array.from({ length: 513 }, (_, index) => `file-${String(index).padStart(4, '0')}.txt`);
  const { files } = await indexed(t, host);
  await files.window(0, 512);
  const selection = files.selection();
  await selection.ready;
  await selection.set(files.items);
  assert.equal(selection.status.current.count, 512);
  await files.window(512, 1);
  const captures = host.requests('selection.set').length;
  assert.throws(() => selection.set([...selection.items, files.items[0]]), hasCode('selection_limit'));
  const foreign = structuredClone(files.items[0]);
  foreign.handle.id = 'foreign-capability';
  assert.throws(() => selection.set([foreign]), hasCode('invalid_selection'));
  assert.equal(host.requests('selection.set').length, captures, 'Invalid selections never cross IPC');
  await selection.set(selection.items);
  assert.equal(selection.status.current.count, 512, 'Prior leases survive page-owner release');
  assert.ok(host.maxPageOwners <= 1024);
});

test('local collection identity and derivation use bounded native record batches in the owning child scope', async t => {
  const { app, host } = application(t);
  const child = app.createScope();
  const alpha = { value: 'alpha' };
  const beta = { value: 'beta' };
  const records = child.collection([alpha, beta]);
  const filtered = records.filter(item => item.value.startsWith('a'));
  const selection = filtered.selection();
  selection.set([alpha]);
  const task = selection.run(app.operation('records.echo'));
  await task.ready;
  assert.equal(host.requests('task.batch').at(-1).request.scope, await child.scope.ready);
  assert.deepEqual(host.tasks.get(task.snapshot.id).inputs, [{ value: 'alpha' }]);
  records.set([beta]);
  assert.deepEqual(filtered.items, []);
  assert.deepEqual(selection.items, []);
  host.complete(task.snapshot.id, { rows: [{ ordinal: '0', input: alpha, output: 'alpha' }] });
  const batch = await task.result;
  assert.equal((await batch.page()).items[0].output, 'alpha', 'Admitted record batch retains its frozen inputs');
  await child.disposeAsync();
  assert.equal(records.snapshot.state, 'disposed');
  assert.equal(host.tasks.size, 0);
});

test('settings edit during loading survives flush and scope teardown waits for atomic persistence', async t => {
  const { app, host } = application(t);
  host.settings.set('files', { search: 'persisted old value' });
  const load = host.blockNext('settings.load', 'reply');
  const save = host.blockNext('settings.save');
  const settings = app.settings.define({ search: '' }, { key: 'files' });
  await load.entered.promise;
  settings.set({ search: 'edit during loading' });
  const flushed = settings.flush();
  const closing = app.disposeAsync();
  load.release.resolve();
  await save.entered.promise;
  assert.equal(host.scopes.size, 1, 'Native scope stays alive during its save queue');
  assert.equal(host.settings.get('files').search, 'persisted old value');
  save.release.resolve();
  await within(Promise.all([flushed, closing]), 'settings save before scope close');
  assert.equal(host.settings.get('files').search, 'edit during loading');
  const saved = host.events.findIndex(event => event.phase === 'persisted');
  const closed = host.events.findIndex(event => event.phase === 'closed');
  assert.ok(saved < closed);
});

test('settings coalesce edits during saving and retry a classified persistence failure', async t => {
  const { app, host } = application(t);
  const settings = app.settings.define({ search: '' }, { key: 'files' });
  await settings.ready;
  const first = host.blockNext('settings.save');
  settings.set({ search: 'first' });
  await first.entered.promise;
  settings.set({ search: 'intermediate' });
  settings.set({ search: 'latest' });
  const flush = settings.flush();
  first.release.resolve();
  await flush;
  assert.deepEqual(host.requests('settings.save').map(call => call.request.value.search), ['first', 'latest']);
  const failed = host.blockNext('settings.save');
  settings.set({ search: 'retry me' });
  await failed.entered.promise;
  const rejected = assert.rejects(settings.flush(), hasCode('disk_full'));
  failed.release.reject(nativeError('disk_full', 'Disk full', true));
  await rejected;
  assert.equal(settings.value.search, 'retry me');
  assert.equal(settings.status.current.state, 'failed');
  await settings.flush();
  assert.equal(host.settings.get('files').search, 'retry me');
});

test('native provider inspection preserves SDK fields and decimal counters; config stays typed and validated', async t => {
  const { app, host } = application(t);
  const [provider] = await app.runtime.inspect();
  assert.equal(provider.activeTasks, '9007199254740993');
  assert.equal(provider.resources, '1');
  assert.equal(provider.configurationSchema.type, 'object');
  await assert.rejects(app.runtime.configure(provider, { enabled: 'wrong type' }), hasCode('contract_violation'));
  assert.equal(host.requests('runtime.configure').length, 0);
  await app.runtime.configure(provider, { enabled: false });
  assert.deepEqual(app.runtime.snapshot[0].config, { enabled: false });
  assert.equal(app.runtime.snapshot[0].generation, 2);
});

test('incompatible compiled root prevents capability work and reclaims the acquired native scope', async t => {
  const { app, host } = application(t, new NativeHost(), { manifest: { ...manifest, digest: `sha256:${'b'.repeat(64)}` } });
  const open = app.files.openFolder();
  await assert.rejects(app.ready, hasCode('incompatible_contract'));
  await assert.rejects(open, hasCode('incompatible_contract'));
  await app.disposeAsync();
  assert.equal(host.requests('folder.open').length, 0);
  assert.equal(host.scopes.size, 0);
});
