import assert from 'node:assert/strict';
import { test } from 'node:test';
import { register } from 'node:module';
import { JSDOM } from 'jsdom';
import { Scope } from '../../src/scope.ts';
import { OverlayCoordinator } from '../../src/overlay.ts';
import { deferred, until, turn } from '../fixtures/native-host.mjs';

register('../fixtures/svelte-loader.mjs', import.meta.url);
const dom = new JSDOM('<!doctype html><html><body><button id="focus">Focus</button></body></html>', { pretendToBeVisual: true });
for (const name of ['window', 'document', 'HTMLElement', 'Element', 'Node', 'Text', 'Comment', 'Event', 'MouseEvent', 'HTMLDialogElement', 'MutationObserver'])
  globalThis[name] = dom.window[name];
globalThis.getComputedStyle = node => ({ ...dom.window.getComputedStyle(node), transform: 'none', perspective: 'none' });
globalThis.ResizeObserver = class { observe() {} disconnect() {} };
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame.bind(dom.window);
globalThis.cancelAnimationFrame = dom.window.cancelAnimationFrame.bind(dom.window);
dom.window.HTMLDialogElement.prototype.showModal = function () { this.open = true; };
dom.window.HTMLDialogElement.prototype.close = function () { this.open = false; this.dispatchEvent(new dom.window.Event('close')); };
const { mount, unmount, flushSync } = await import('svelte');

test('native workflow reference page mounts with reads and overlays in its component scope', async () => {
  const { createApp, contract } = await import('../fixtures/NativeWorkflowFacade.mjs');
  const Page = (await import('../fixtures/NativeWorkflowPage.svelte')).default;
  const requests = [];
  let sequence = 0;
  const app = createApp({ transport: { dispatch: async request => {
    requests.push(request);
    if (request.action === 'root.connect') return { scope: 'root', manifest: contract };
    if (request.action === 'scope.create') return { scope: `component-${++sequence}` };
    return null;
  } } });
  let page;
  try {
    page = mount(Page, { target: document.body, props: { app } });
    flushSync();
    await app.ready;
    await until(() => requests.some(request => request.action === 'scope.create'), 'component scope');
    assert.ok(document.querySelector('h1')?.textContent.includes('Native workflows'));
    assert.equal(requests.filter(request => request.action === 'scope.create').length, 1);
    await unmount(page);
    page = undefined;
    await until(() => requests.some(request => request.action === 'scope.dispose' && request.scope === 'component-1'), 'component cleanup');
  } finally {
    if (page) await unmount(page);
    await app.disposeAsync();
  }
});

test('mounted native canvases and HyvUI dialogs obey hide/show acknowledgement and manual suspension', async t => {
  const NativeCanvas = (await import('../../src/ui/NativeCanvas.svelte')).default;
  const CoveredModal = (await import('../../src/ui/CoveredModal.svelte')).default;
  const requests = [];
  const hide = deferred();
  let blockHide = false;
  const scope = new Scope({ dispatch: async request => {
    requests.push(request);
    if (request.action === 'surface.attach') return { viewport: 'v', adapter: 'fixture', bufferBytes: 1 };
    if (request.action === 'surface.place' && !request.visible && blockHide) await hide.promise;
    return null;
  } }, Promise.resolve('root'));
  t.after(() => scope.disposeAsync());
  const overlays = new OverlayCoordinator(scope);
  const canvas = mount(NativeCanvas, { target: document.body, props: { scope, scene: { id: 'scene' }, overlays } });
  flushSync();
  await until(() => requests.some(r => r.action === 'surface.place' && r.visible), 'visible canvas');
  blockHide = true;
  document.getElementById('focus').focus();
  const first = mount(CoveredModal, { target: document.body, props: { overlays, open: true, title: 'First' } });
  flushSync();
  await until(() => requests.some(r => r.action === 'surface.place' && !r.visible), 'pending hide');
  assert.equal(document.querySelector('dialog'), null);
  hide.resolve();
  await until(() => document.querySelector('dialog')?.open, 'covered modal');
  const second = mount(CoveredModal, { target: document.body, props: { overlays, open: true, title: 'Second' } });
  flushSync();
  await until(() => document.querySelectorAll('dialog[open]').length === 2, 'nested modal');
  await unmount(first);
  await turn();
  const beforeClose = requests.length;
  assert.ok(overlays.covered);
  await canvas.suspend();
  await unmount(second);
  await until(() => !overlays.covered, 'last cover removed');
  assert.equal(document.querySelector('dialog'), null);
  assert.ok(!requests.slice(beforeClose).some(r => r.action === 'surface.place' && r.visible));
  await canvas.restore();
  await until(() => requests.slice(beforeClose).some(r => r.action === 'surface.place' && r.visible), 'manual restoration');
  assert.equal(document.activeElement.id, 'focus');
  await unmount(canvas);
  await until(() => requests.some(r => r.action === 'surface.dispose'), 'canvas teardown acknowledgement');
  assert.equal(requests.filter(r => r.action === 'surface.dispose').length, 1);
});

test('mounted group reader freezes its result and mounted dialog closes on root teardown', async t => {
  const Reader = (await import('../fixtures/GroupReader.svelte')).default;
  const CoveredModal = (await import('../../src/ui/CoveredModal.svelte')).default;
  const { queryGroup } = await import('../../src/query-group.ts');
  const { callableOperation } = await import('../../src/operation.ts');
  const { createApp } = await import('../../src/app.ts');
  const { NativeHost, manifest } = await import('../fixtures/native-host.mjs');
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  t.after(() => app.disposeAsync());
  const read = callableOperation(app.bindOperation(manifest.operations[0]), true);
  const group = queryGroup(app.scope, { required: { title: read }, optional: { extra: read } });
  const overlays = new OverlayCoordinator(app.scope);
  const reader = mount(Reader, { target: document.body, props: { group } });
  const dialog = mount(CoveredModal, { target: document.body, props: { overlays, open: true, title: 'Teardown' } });
  flushSync();
  await until(() => document.querySelector('dialog')?.open, 'root dialog');
  const loading = group.load('active', { title: { value: 'title' }, extra: { value: 'extra' } });
  await until(() => host.tasks.size === 2, 'mounted slots');
  const [title, optional] = [...host.tasks.keys()];
  host.complete(optional, { result: 'extra' });
  await turn(); flushSync();
  assert.equal(document.querySelector('[data-result]').textContent, 'loading::');
  host.complete(title, { result: 'title' });
  await loading; flushSync();
  assert.equal(document.querySelector('[data-result]').textContent, 'ready:active:title');
  await app.disposeAsync();
  await turn(); flushSync();
  assert.equal(document.querySelector('dialog'), null);
  assert.equal(document.querySelector('[data-result]').textContent, 'ready:active:title');
  await unmount(reader); await unmount(dialog);
});
