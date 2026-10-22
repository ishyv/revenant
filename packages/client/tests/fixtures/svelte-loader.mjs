import { readFile } from 'node:fs/promises';
import { compile } from 'svelte/compiler';

// Compile the actual source and its vendor components, never extracted script fragments.
export async function resolve(specifier, context, nextResolve) {
  if (specifier === '#lib/revenant.ts' && context.parentURL.endsWith('/examples/native-workflows/web/src/routes/+page.svelte'))
    return { url: new URL('./NativeWorkflowFacade.mjs', import.meta.url).href, shortCircuit: true };
  // Reference-example components must share this harness's Svelte and client
  // instances rather than loading their separately materialized SDK snapshot.
  if (specifier === 'svelte' || specifier.startsWith('svelte/') ||
      specifier === '@revenant/client' || specifier.startsWith('@revenant/client/'))
    return nextResolve(specifier, { ...context, parentURL: new URL('../../package.json', import.meta.url).href });
  return nextResolve(specifier, context);
}
export async function load(url, context, nextLoad) {
  if (url.endsWith('.svelte')) {
    const source = await readFile(new URL(url), 'utf8');
    return { format: 'module', shortCircuit: true,
      source: compile(source, { filename: new URL(url).pathname, generate: 'client', css: 'injected' }).js.code };
  }
  return nextLoad(url, context);
}
