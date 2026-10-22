// Scope-affinity fixture using the real public runtime and Svelte context helpers.
// Generated contract parity and packaged application checks run separately.
import { createApp as createRuntime, callableOperation } from '../../src/index.ts';
import { provideApp, useApp as useRuntimeApp } from '../../src/svelte.ts';
export { provideApp };

export const contract = {
  version: 6, protocol: 5, digest: `sha256:${'b'.repeat(64)}`,
  operations: ['build', 'optional', 'patch', 'scene', 'summary'].map(name => ({
    id: `fixture.${name}`, description: 'Component scope fixture', arguments: [],
    effect: ['optional', 'summary'].includes(name) ? 'readOnly' : 'write',
    workClass: 'cpu', independent: false, agent: false,
    input: { name: 'Input', typescript: 'unknown', schema: {} },
    output: { name: 'Output', typescript: 'unknown', schema: {} },
    source: { file: 'native/fixture.rs', line: 1, module: 'fixture' },
  })),
};

const bound = new WeakSet();
function bindApp(runtime) {
  if (bound.has(runtime)) return runtime;
  bound.add(runtime);
  const createScope = runtime.createScope.bind(runtime);
  return Object.assign(runtime, {
    createScope: () => bindApp(createScope()),
    operations: { fixture: Object.fromEntries(contract.operations.map(definition =>
      [definition.id.split('.')[1], runtime.bindOperation(definition)])) },
  });
}
export function createApp(options) { return bindApp(createRuntime({ ...options, manifest: contract })); }
export function useApp(app) { return bindApp(useRuntimeApp(app)); }
// Keep the existing helper's child-scope behavior so the original page fails.
export function useOperations(app) {
  const runtime = useApp(app);
  return { fixture: Object.fromEntries(Object.entries(runtime.operations.fixture).map(([name, operation]) =>
    [name, callableOperation(operation)])) };
}
