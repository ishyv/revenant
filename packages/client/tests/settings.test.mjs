import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Settings } from '../src/settings.ts';
import { Scope } from '../src/scope.ts';

function settings(t, defaults, loaded) {
  const saves = [];
  const owner = new Scope({ async dispatch(request) {
    if (request.action === 'settings.load') return loaded;
    if (request.action === 'settings.save') saves.push(request.value);
  } }, Promise.resolve('settings-owner'));
  const store = new Settings(owner, defaults, { key: 'workspace' });
  t.after(() => owner.disposeAsync());
  return { store, saves };
}

test('settings load and save preserve unknown persisted object fields', async t => {
  const { store, saves } = settings(t, { theme: 'dark', panel: { width: 20 } },
    { theme: 'light', panel: { width: 40, obsolete: true }, legacy: 7 });
  await store.ready;
  store.update(value => ({ ...value, theme: 'dark' }));
  await store.flush();
  assert.deepEqual(saves, [{ theme: 'dark', panel: { width: 40, obsolete: true }, legacy: 7 }]);
});

test('settings arrays compare declared positions and allow native-preserved tails', async t => {
  const { store, saves } = settings(t, { tuple: ['name', 1], empty: [] },
    { tuple: ['updated', 2, { extra: true }], empty: [false] });
  await store.ready;
  store.set({ tuple: ['changed', 3], empty: ['anything'] });
  await store.flush();
  assert.deepEqual(saves, [{ tuple: ['changed', 3], empty: ['anything'] }]);
  store.set({ tuple: ['short'], empty: [] });
  await store.flush();
  assert.deepEqual(store.value, { tuple: ['short'], empty: [] });
});

test('settings still reject incorrect known properties, array positions and non-JSON', async t => {
  const { store } = settings(t, { theme: 'dark', tuple: ['name', 1] },
    { theme: 'dark', tuple: ['name', 1] });
  await store.ready;
  for (const value of [{ theme: 1, tuple: ['name', 1] },
    { theme: 'dark', tuple: ['name', 'wrong'] }, { tuple: ['name', 1] }]) {
    assert.throws(() => store.set(value), error => error.code === 'invalid_settings');
  }
  assert.throws(() => store.set({ theme: 'dark', tuple: ['name', 1], extra: undefined }),
    error => error.code === 'invalid_json');
});
