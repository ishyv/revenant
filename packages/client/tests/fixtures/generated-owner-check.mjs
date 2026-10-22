// Exercise the actual published facade, with real runtime scopes and Svelte hooks.
import assert from 'node:assert/strict';
import { register } from 'node:module';
import { pathToFileURL } from 'node:url';
import { JSDOM } from 'jsdom';
import { queryGroup } from '../../src/query-group.ts';
import { until } from './native-host.mjs';

register('./svelte-loader.mjs', import.meta.url);
const dom = new JSDOM('<!doctype html><html><body></body></html>');
for (const name of ['window', 'document', 'HTMLElement', 'Element', 'Node', 'Text', 'Comment', 'Event'])
  globalThis[name] = dom.window[name];
const { mount, unmount, flushSync } = await import('svelte');
const Reader = (await import('./ContextOperationReader.svelte')).default;
const facade = await import(pathToFileURL(process.argv[2]).href);
const requests = [];
let sequence = 0;
const app = facade.createApp({ transport: { dispatch: async request => {
  requests.push(request);
  if (request.action === 'root.connect') return { scope: 'root', manifest: facade.contract };
  if (request.action === 'scope.create') return { scope: `child-${++sequence}` };
  return null;
} } });
let reader;
try {
  await app.ready;
  const owner = app.createScope();
  await owner.ready;
  // An explicitly owned controller runs after initialization, outside Svelte.
  const explicit = facade.useOperations(owner).records.normalize;
  assert.equal(explicit.owner, owner.scope);
  const group = queryGroup(owner.scope, { required: { title: explicit } });
  await group.dispose();
  await until(() => requests.some(r => r.action === 'scope.create'), 'explicit scope');
  assert.equal(requests.filter(r => r.action === 'scope.create').length, 1);
  let componentRead;
  reader = mount(Reader, { target: document.body, props: { read: () => {
    facade.provideApp(app);
    componentRead = facade.useOperations().records.normalize;
  } } });
  flushSync();
  assert.notEqual(componentRead.owner, app.scope);
  await until(() => requests.filter(r => r.action === 'scope.create').length === 2, 'component scope');
  assert.equal(componentRead.owner.disposed, false);
  await unmount(reader);
  reader = undefined;
  await until(() => requests.some(r => r.action === 'scope.dispose' && r.scope === 'child-2'), 'component cleanup');
  assert.equal(componentRead.owner.disposed, true);
  assert.equal(owner.scope.disposed, false);
  await owner.disposeAsync();
} finally {
  if (reader) await unmount(reader);
  await app.disposeAsync();
  dom.window.close();
}
console.log('Published facade: explicit scope exact; default component scope disposed.');
