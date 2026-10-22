/** Node-only development integration: invalidate immutable optimized SDK imports. */
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export function revenantSdk() {
  const root = fileURLToPath(new URL('.', import.meta.url));
  const hash = createHash('sha256');
  function add(relative) {
    const bytes = readFileSync(join(root, relative));
    hash.update(`${relative.replaceAll('\\', '/')}\0${bytes.length}\0`);
    hash.update(bytes);
  }
  function visit(relative) {
    for (const entry of readdirSync(join(root, relative), { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name, 'en'))) {
      const path = join(relative, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile()) add(path);
      else throw new Error(`Unsupported SDK source entry: ${path}`);
    }
  }
  add('package.json');
  add('vite.js');
  visit('src');
  // Vite includes plugin names in its optimizer config hash. Package versions
  // and dependency paths alone do not identify a replaced local SDK's source.
  return { name: `revenant-sdk-${hash.digest('hex')}` };
}
