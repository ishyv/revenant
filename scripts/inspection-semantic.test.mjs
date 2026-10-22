import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { inspectSemantic } from './inspection-semantic.mjs';

const dependencies = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../packages/client');

test('The language server uses the application pinned Svelte compiler',()=>{
  const client=createRequire(path.join(dependencies,'package.json'));
  const server=createRequire(client.resolve('svelte-language-server/package.json'));
  const pinned=JSON.parse(fs.readFileSync(path.join(dependencies,'package.json'),'utf8')).devDependencies.svelte;
  assert.equal(server('svelte/package.json').version,pinned,'the language server must not retain a vulnerable fallback compiler');
});
function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'revenant-semantic-'));
  fs.mkdirSync(path.join(root, 'web/src/lib/.revenant/current'), { recursive:true });
  fs.mkdirSync(path.join(root, 'web/src/routes'), { recursive:true });
  fs.writeFileSync(path.join(root, 'web/package.json'), '{"type":"module"}');
  fs.writeFileSync(path.join(root, 'web/tsconfig.json'), JSON.stringify({compilerOptions:{module:'ESNext',moduleResolution:'bundler',target:'ESNext',strict:true,baseUrl:'.',paths:{'$lib/*':['src/lib/*']}}}));
  fs.writeFileSync(path.join(root, 'web/src/lib/.revenant/current/facade.ts'), 'export interface Operations { titles: { normalize: (input: { text: string }) => Promise<string> } }\nexport declare function useOperations(): Operations;\n');
  fs.writeFileSync(path.join(root, 'web/src/lib/revenant.ts'), "export * from './.revenant/current/facade';\n");
  fs.writeFileSync(path.join(root, 'web/src/lib/consumer.ts'), "import { useOperations as useNative } from '$lib/revenant';\nconst { titles: renamed } = useNative();\nexport const result = renamed.normalize({ text: 'hello' });\n");
  return root;
}

test('TypeScript semantic references follow import aliases and destructuring', async () => {
  const root = fixture();
  try {
    const result = await inspectSemantic({project:root,dependencyRoot:dependencies,action:'operation',operation:'titles.normalize',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:30,timeoutMs:15000});
    assert.ok(result.references.some(ref => ref.file === 'web/src/lib/consumer.ts' && ref.line === 3), JSON.stringify(result));
    assert.ok(result.references.every(ref => ['typescript_language_service','svelte_language_server'].includes(ref.provenance)));
    assert.ok(result.binding.file.endsWith('/facade.ts'));
  } finally { fs.rmSync(root, {recursive:true,force:true}); }
});

test('Svelte semantic references map component usage back to authored lines', async () => {
  const root = fixture();
  fs.writeFileSync(path.join(root, 'web/src/routes/+page.svelte'), "<script lang=\"ts\">\nimport { useOperations } from '$lib/revenant';\nconst api = useOperations();\nconst normalizeTitle = api.titles.normalize;\n</script>\n<button onclick={() => normalizeTitle({ text: 'hi' })}>Normalize</button>\n");
  try {
    const result = await inspectSemantic({project:root,dependencyRoot:dependencies,action:'operation',operation:'titles.normalize',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:30,timeoutMs:25000});
    assert.ok(result.references.some(ref => ref.file === 'web/src/routes/+page.svelte' && ref.line === 4 && ref.provenance === 'svelte_language_server'), JSON.stringify(result));
  } finally { fs.rmSync(root, {recursive:true,force:true}); }
});

test('Unavailable language tools produce explicit incomplete indexing', async () => {
  const root = fixture();
  try {
    const result = await inspectSemantic({project:root,dependencyRoot:path.join(root,'absent'),action:'operation',operation:'titles.normalize',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:3,timeoutMs:500});
    assert.equal(result.status, 'unavailable');
    assert.deepEqual(result.references, []);
    assert.ok(result.issues.some(issue => issue.service === 'typescript'));
  } finally { fs.rmSync(root, {recursive:true,force:true}); }
});

test('Compiler diagnostics retain stable codes and authored positions', async () => {
  const root = fixture();
  fs.writeFileSync(path.join(root,'web/src/lib/consumer.ts'), "const title: string = 42;\nexport { title };\n");
  try {
    const result=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'diagnostics',maxResults:10});
    assert.ok(result.diagnostics.some(item=>item.code==='2322' && item.stage==='typescript' && item.file==='web/src/lib/consumer.ts' && item.line===1));
  } finally { fs.rmSync(root, {recursive:true,force:true}); }
});

test('Semantic file queries reject escaping paths and invalid result budgets', async () => {
  const root=fixture();
  try {
    await assert.rejects(inspectSemantic({project:root,action:'query',file:'../outside.ts',line:1,column:1,maxResults:201}),/maxResults/);
    const result=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'query',file:'../../outside.ts',line:1,column:1,maxResults:5});
    assert.equal(result.status,'partial');
    assert.equal(result.references.length,0);
    assert.ok(result.issues.length>0);
  } finally { fs.rmSync(root, {recursive:true,force:true}); }
});

test('Managed-process entrypoint emits one JSON result without source edits', () => {
  const root=fixture();
  try {
    const source=fs.readFileSync(new URL('./inspection-semantic.mjs',import.meta.url),'utf8');
    const output=execFileSync(process.execPath,['--input-type=module','--eval',source,'--','--request-env'],{env:{...process.env,REVENANT_INSPECT_REQUEST:JSON.stringify({project:root,dependencyRoot:dependencies,action:'operation',operation:'titles.normalize',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:20})},encoding:'utf8',timeout:15000});
    const result=JSON.parse(output);
    assert.ok(result.references.some(ref=>ref.file==='web/src/lib/consumer.ts'));
  } finally { fs.rmSync(root, {recursive:true,force:true}); }
});

test('Svelte diagnostic checks report complete component coverage', async () => {
  const root=fixture();
  fs.writeFileSync(path.join(root,'web/src/routes/+page.svelte'),'<script lang="ts">\nconst title: string = 42;\n</script>\n<p>{title}</p>\n');
  try {
    const result=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'diagnostics',maxResults:20,timeoutMs:20000});
    assert.ok(result.diagnostics.some(item=>item.file==='web/src/routes/+page.svelte' && item.code==='2322' && item.line===2),JSON.stringify(result));
    assert.equal(result.services.find(service=>service.name==='svelte_language_server').indexedComponents,1);
  } finally {fs.rmSync(root,{recursive:true,force:true,maxRetries:10,retryDelay:100});}
});

test('Callable useOperations bindings resolve Svelte read helpers and direct calls semantically',async()=>{
  const root=fixture();
  fs.writeFileSync(path.join(root,'web/src/lib/.revenant/current/facade.ts'),"export type ReadOperationFunction<I, O> = ((input: I) => Promise<O>) & { run(input: I): { result(): Promise<O> }; query(input: I): { value: O | undefined } };\nexport interface Application { operations: { titles: { normalize: { run(input: { text: string }): unknown } } } }\nexport interface Operations { titles: { normalize: ReadOperationFunction<{ text: string }, string> } }\nexport declare function useOperations(app?: Application): Operations;\n");
  fs.writeFileSync(path.join(root,'web/src/routes/+page.svelte'),"<script lang=\"ts\">\nimport { useOperations as connect } from '$lib/revenant';\nconst api = connect();\nconst { normalize: tidy } = api.titles;\nconst direct = api.titles.normalize({ text: 'hello' });\nconst query = api.titles.normalize.query({ text: 'hello' });\n</script>\n<button onclick={() => tidy.run({ text: 'hi' })}>Normalize</button>\n");
  try {
    const result=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'operation',operation:'titles.normalize',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:30,timeoutMs:25000});
    assert.equal(result.binding.line,3);
    assert.ok(result.references.some(ref=>ref.file==='web/src/routes/+page.svelte'&&ref.line===4),JSON.stringify(result));
    assert.ok(result.references.some(ref=>ref.file==='web/src/routes/+page.svelte'&&ref.line===5));
    assert.ok(result.references.some(ref=>ref.file==='web/src/routes/+page.svelte'&&ref.line===6));
  } finally {fs.rmSync(root,{recursive:true,force:true});}
});

test('Svelte operation indexing finds a destructured function without a spelled member access',async()=>{
  const root=fixture();
  fs.writeFileSync(path.join(root,'web/src/routes/+page.svelte'),"<script lang=\"ts\">\nimport { useOperations } from '$lib/revenant';\nconst { titles: { normalize: tidy } } = useOperations();\n</script>\n<button onclick={() => tidy({ text: 'hi' })}>Normalize</button>\n");
  try {
    const result=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'operation',operation:'titles.normalize',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:30,timeoutMs:25000});
    assert.ok(result.references.some(ref=>ref.file==='web/src/routes/+page.svelte'&&ref.line===3),JSON.stringify(result));
  } finally {fs.rmSync(root,{recursive:true,force:true});}
});

test('Operation queries union callable and legacy application symbols in TypeScript and Svelte',async()=>{
  const root=fixture();
  fs.writeFileSync(path.join(root,'web/src/lib/.revenant/current/facade.ts'),"type Operation<I,O> = { run(input:I): { result():Promise<O> } };\nexport interface Application { operations: { titles: { normalize: Operation<{text:string},string> } }; files: { checksum: Operation<{file:string},string> }; media: { readMetadata: Operation<{file:string},string> } }\nexport interface Operations { titles: { normalize: (input:{text:string})=>Promise<string> }; files: { checksum: (input:{file:string})=>Promise<string> }; media: { readMetadata: (input:{file:string})=>Promise<string> } }\nexport declare function useApp():Application;\nexport declare function useOperations():Operations;\n");
  fs.writeFileSync(path.join(root,'web/src/lib/consumer.ts'),"import { useOperations, useApp } from '$lib/revenant';\nconst api = useOperations();\nconst app = useApp();\nexport const modern = api.titles.normalize({ text: 'hello' });\nexport const legacy = app.operations.titles.normalize.run({ text: 'hello' });\nexport const checksum = app.files.checksum.run({file:'file'});\nexport const metadata = app.media.readMetadata.run({file:'file'});\n");
  fs.writeFileSync(path.join(root,'web/src/routes/+page.svelte'),"<script lang=\"ts\">\nimport { useApp, useOperations } from '$lib/revenant';\nconst app = useApp(); const api = useOperations();\nconst modern = api.titles.normalize({ text: 'hello' });\nconst legacy = app.operations.titles.normalize.run({ text: 'hello' });\nconst checksum = app.files.checksum.run({file:'file'});\nconst metadata = app.media.readMetadata.run({file:'file'});\n</script>\n<p>Operation consumers</p>\n");
  try {
    const custom=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'operation',operation:'titles.normalize',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:40,timeoutMs:25000});
    for(const file of ['web/src/lib/consumer.ts','web/src/routes/+page.svelte'])for(const line of [4,5])assert.ok(custom.references.some(ref=>ref.file===file&&ref.line===line),JSON.stringify(custom));
    assert.ok(custom.bindings.length>=2);
    const checksum=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'operation',operation:'files.checksum',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:40,timeoutMs:25000});
    const metadata=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'operation',operation:'media.readMetadata',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:40,timeoutMs:25000});
    for(const [result,line] of [[checksum,6],[metadata,7]])for(const file of ['web/src/lib/consumer.ts','web/src/routes/+page.svelte'])assert.ok(result.references.some(ref=>ref.file===file&&ref.line===line),JSON.stringify(result));
    assert.ok(custom.references.every(ref=>ref.binding&&['typescript_language_service','svelte_language_server'].includes(ref.provenance)));
  } finally {fs.rmSync(root,{recursive:true,force:true});}
});

test('Source discovery stops at an entry budget and marks semantic results partial',async()=>{
  const root=fixture();const directory=path.join(root,'web/src/many');fs.mkdirSync(directory);
  for(let index=0;index<8300;index++)fs.writeFileSync(path.join(directory,`entry${String(index).padStart(5,'0')}.json`),'{}');
  try {
    const result=await inspectSemantic({project:root,dependencyRoot:dependencies,action:'operation',operation:'titles.normalize',facade:'web/src/lib/.revenant/current/facade.ts',maxResults:10});
    assert.equal(result.status,'partial');
    assert.equal(result.truncated,true);
    assert.ok(result.sourceScan.visitedEntries<=8192);
    assert.ok(result.sourceScan.indexedFiles<=4096);
  } finally {fs.rmSync(root,{recursive:true,force:true});}
});

test('rust-analyzer resolves a real Rust definition and reference',{skip:!process.env.REVENANT_TEST_RUST_ANALYZER},async()=>{
  const root=fixture();
  fs.mkdirSync(path.join(root,'native/src'),{recursive:true});
  fs.writeFileSync(path.join(root,'native/Cargo.toml'),'[package]\nname = "inspection_native"\nversion = "0.1.0"\nedition = "2024"\n');
  fs.writeFileSync(path.join(root,'native/src/lib.rs'),'pub fn normalize(text: &str) -> &str { text.trim() }\npub fn example() -> &\'static str {\n    normalize(" hello ")\n}\n');
  try {
    const result=await inspectSemantic({project:root,action:'query',file:'native/src/lib.rs',line:3,column:5,maxResults:10,timeoutMs:25000});
    assert.ok(result.definitions.some(item=>item.file==='native/src/lib.rs'&&item.line===1&&item.provenance==='rust_analyzer'),JSON.stringify(result));
    assert.ok(result.references.some(item=>item.file==='native/src/lib.rs'&&item.line===3));
    assert.equal(result.services.find(service=>service.name==='rust_analyzer').macroExpansion,'disabled');
    assert.equal(result.status,'partial');
    const symbols=await inspectSemantic({project:root,action:'query',symbol:'normalize',file:'native/src/lib.rs',maxResults:10,timeoutMs:25000});
    assert.equal(typeof symbols.status,'string');
    assert.ok(symbols.symbols.some(item=>item.name==='normalize'&&item.file==='native/src/lib.rs'),JSON.stringify(symbols));
  } finally {fs.rmSync(root,{recursive:true,force:true});}
});
