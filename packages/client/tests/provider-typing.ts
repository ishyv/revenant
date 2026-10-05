import type { App, FileEntry, OperationDefinition, OperationInput, OperationOutput, ProviderDescriptor, Query, Task } from '../src/index.js';
import { useApp } from '../src/svelte.js';

type Input = { text: string };
type Output = { length: number };
declare const app: App;
declare const registration: OperationDefinition;
const operation = app.bindOperation<Input, Output>(registration);
const result: Promise<Output> = operation.call({ text: 'native' });
const task: Task<Output> = operation.run({ text: 'native' });
const query: Query<Input, Output> = operation.query();
const queryInput: OperationInput<typeof operation> = { text: 'query' };
const queryOutput: OperationOutput<typeof operation> = { length: 5 };
const loading: Promise<void> = query.load(queryInput);
// @ts-expect-error Query inputs preserve the compiled contract.
void query.load({ text: 12 });
// @ts-expect-error Output helpers preserve generated property types.
const wrongOutput: OperationOutput<typeof operation> = { length: 'five' };
void [queryOutput, loading, wrongOutput];
// @ts-expect-error Compiled operation inputs retain the generated field type.
operation.run({ text: 12 });
// @ts-expect-error Native execution callbacks are not client provider APIs.
operation.provider({ execute: () => ({ length: 0 }) });

const provider: ProviderDescriptor<{ enabled: boolean }> = {
  id: 'native.fixture', description: 'Compiled native fixture provider', dependencies: [],
  configurationSchema: {
    type: 'object', properties: { enabled: { type: 'boolean' } }, required: ['enabled'],
  },
};
void app.runtime.configure(provider, { enabled: true });
// @ts-expect-error Native configuration retains the declared provider config type.
void app.runtime.configure(provider, { enabled: 'yes' });
// @ts-expect-error Replacements name native factories; JavaScript execution callbacks are forbidden.
void app.runtime.replace(provider, { id: 'replacement', execute: () => {} });

type Checksum = { digest: string };
type Metadata = { mime: string };
declare const compiledApp: App<Checksum, Metadata>;
declare const entry: FileEntry;
const checksum: Promise<Checksum> = compiledApp.files.checksum.call(entry);
const metadata: Promise<Metadata> = compiledApp.media.readMetadata.call(entry);
const child = compiledApp.createScope();
const childChecksum: Promise<Checksum> = child.files.checksum.call(entry);
const scoped = useApp(compiledApp);
const scopedMetadata: Promise<Metadata> = scoped.media.readMetadata.call(entry);
void [result, task, checksum, metadata, childChecksum, scopedMetadata];
