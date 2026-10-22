import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApp } from '../src/app.ts';
import { NativeHost, manifest, hasCode } from './fixtures/native-host.mjs';

test('resource release sends the complete handle through its owning application scope', async t => {
  const host = new NativeHost();
  const requests = [];
  const transport = { dispatch: async (request, updates) => {
    if (request.action === 'resource.release') { requests.push(request); return null; }
    return host.dispatch(request, updates);
  } };
  const app = createApp({ transport, manifest });
  t.after(() => app.disposeAsync());
  const child = app.createScope();
  await child.ready;
  await child.scope.ready;
  const handle = { id: 'native-1', scope: child.scope.id, generation: 7, kind: 'native:Plan' };
  assert.equal(await child.releaseResource(handle), undefined);
  assert.deepEqual(requests, [{ action: 'resource.release', scope: child.scope.id, handle }]);
  await child.disposeAsync();
  await assert.rejects(child.releaseResource(handle), hasCode('scope_disposed'));
  assert.equal(requests.length, 1);
});

test('resource release preserves native validation errors for callers', async t => {
  const host = new NativeHost();
  const transport = { dispatch: async (request, updates) => {
    if (request.action === 'resource.release') throw { code: 'stale_handle', message: 'Owner was already released' };
    return host.dispatch(request, updates);
  } };
  const app = createApp({ transport, manifest });
  t.after(() => app.disposeAsync());
  await app.ready;
  await assert.rejects(app.releaseResource({ id: 'r-1', scope: app.scope.id, generation: 1, kind: 'native:Plan' }), hasCode('stale_handle'));
});
