import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createApp } from '../src/app.ts';
import { NativeHost, hasCode, manifest } from './fixtures/native-host.mjs';

test('typed folder capability is copied separately from display path and revoked on disposal', async () => {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  const folder = await app.files.openFolder();
  const capability = { id: 'native-directory', scope: folder.owner.id, generation: 1, kind: 'native:folder' };
  folder.descriptor.capability = capability;
  const received = folder.capability;
  assert.deepEqual(received, capability);
  assert.notEqual(received, capability);
  received.id = 'mutated-untrusted-copy';
  assert.equal(folder.capability.id, capability.id);
  assert.notEqual(folder.path, received.id);
  await folder.dispose();
  assert.throws(() => folder.capability, hasCode('folder_disposed'));
  await app.disposeAsync();
});

test('older descriptors require native SDK upgrade before exposing typed directory authority', async () => {
  const host = new NativeHost();
  const app = createApp({ transport: host, manifest });
  const folder = await app.files.openFolder();
  assert.throws(() => folder.capability, hasCode('folder_capability_missing'));
  await app.disposeAsync();
});
