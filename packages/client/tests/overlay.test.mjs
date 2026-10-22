import assert from 'node:assert/strict';
import { test } from 'node:test';
import { OverlayCoordinator } from '../src/overlay.ts';
import { Scope } from '../src/scope.ts';
import { deferred, turn, hasCode } from './fixtures/native-host.mjs';

function fixture(t) {
  const scope = new Scope({ dispatch: async () => null }, Promise.resolve('root'));
  t.after(() => scope.disposeAsync());
  return { scope, overlays: new OverlayCoordinator(scope) };
}

test('cover and close await native hiding and content removal; nested tokens restore once', async t => {
  const { overlays } = fixture(t);
  const hide = deferred(), closed = deferred();
  const events = [];
  await overlays.register({ suspend: async () => { events.push('hide'); await hide.promise; }, restore: async () => events.push('restore') }).ready;
  let opened = false;
  const covering = overlays.cover().then(token => { opened = true; return token; });
  await turn();
  assert.equal(opened, false);
  hide.resolve();
  const first = await covering;
  const second = await overlays.cover();
  await first.close(async () => events.push('close-one'));
  assert.deepEqual(events, ['hide', 'close-one']);
  const closing = second.close(async () => { events.push('close-two'); await closed.promise; });
  await turn();
  assert.ok(!events.includes('restore'));
  closed.resolve(); await closing;
  assert.deepEqual(events, ['hide', 'close-one', 'close-two', 'restore']);
});

test('late canvases hide before registration readiness and retired canvases never restore', async t => {
  const { overlays } = fixture(t);
  const token = await overlays.cover();
  const events = [];
  const registration = overlays.register({ suspend: async () => events.push('hide'), restore: async () => events.push('restore') });
  await registration.ready;
  assert.deepEqual(events, ['hide']);
  registration.dispose();
  await token.close(async () => {});
  assert.deepEqual(events, ['hide']);
});

test('failed late registration is retried before a nested cover can open', async t => {
  const { overlays } = fixture(t);
  const first = await overlays.cover();
  const events = [];
  const registration = overlays.register({
    suspend: async () => { events.push('hide'); throw Error('late hide failed'); },
    restore: async () => events.push('restore'),
  });
  await assert.rejects(registration.ready, /late hide failed/);
  await assert.rejects(overlays.cover(), /late hide failed/);
  assert.equal(overlays.covered, true);
  assert.deepEqual(events, ['hide', 'hide']);
  assert.equal(overlays.errors.current.length, 2);
  await first.close(async () => {});
  assert.equal(overlays.covered, false);
  assert.deepEqual(events, ['hide', 'hide', 'restore']);
});

test('a successful late-registration retry awaits hiding acknowledgement', async t => {
  const { overlays } = fixture(t);
  const first = await overlays.cover();
  const hide = deferred();
  let attempts = 0, restores = 0;
  const registration = overlays.register({
    suspend: async () => {
      if (++attempts === 1) throw Error('late hide failed');
      await hide.promise;
    },
    restore: async () => { ++restores; },
  });
  await assert.rejects(registration.ready, /late hide failed/);
  let opened = false;
  const covering = overlays.cover().then(token => { opened = true; return token; });
  await turn();
  assert.equal(attempts, 2);
  assert.equal(opened, false);
  hide.resolve();
  const second = await covering;
  await first.close(async () => {});
  assert.equal(restores, 0);
  await second.close(async () => {});
  assert.equal(restores, 1);
});

test('hide, content-close and restore failures are observable and root closure never restores', async t => {
  const { scope, overlays } = fixture(t);
  const registration = overlays.register({ suspend: async () => { throw Error('hide failed'); }, restore: async () => {} });
  await registration.ready;
  await assert.rejects(overlays.cover(), /hide failed/);
  assert.equal(overlays.errors.current.length, 1);
  registration.dispose();
  await overlays.register({ suspend: async () => {}, restore: async () => { throw Error('restore failed'); } }).ready;
  const token = await overlays.cover();
  await assert.rejects(token.close(async () => { throw Error('close failed'); }), /close failed/);
  assert.equal(overlays.covered, true);
  await assert.rejects(token.close(async () => {}), /restore failed/);
  assert.equal(overlays.errors.current.length, 3);
  const covered = await overlays.cover();
  scope.dispose();
  await covered.close(async () => {});
  assert.throws(() => overlays.cover(), hasCode('scope_disposed'));
});
