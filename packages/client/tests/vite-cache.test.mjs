import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, mkdir, writeFile, copyFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

test('Vite optimizer identity follows SDK client source, not package version or build output', async t => {
  const root = await mkdtemp(join(tmpdir(), 'revenant-vite-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'src'));
  await writeFile(join(root, 'package.json'), '{"type":"module","version":"0.4.0"}');
  await writeFile(join(root, 'src', 'surface.ts'), 'export const scale = 1;');
  await copyFile(new URL('../vite.js', import.meta.url), join(root, 'vite.js'));
  const { revenantSdk } = await import(pathToFileURL(join(root, 'vite.js')).href);
  const initial = revenantSdk().name;
  assert.equal(revenantSdk().name, initial);
  await mkdir(join(root, 'node_modules'));
  await writeFile(join(root, 'node_modules', 'artifact.js'), 'build output');
  assert.equal(revenantSdk().name, initial);
  await writeFile(join(root, 'src', 'surface.ts'), 'export const scale = devicePixelRatio;');
  assert.notEqual(revenantSdk().name, initial);
});
