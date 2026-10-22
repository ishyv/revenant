import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createEventMetrics, summarizeRun, validateConfiguration, fixedCases, execute, verifyCase, prepareCase, verifyProvenance, agentArguments, runEvaluation, buildChildEnvironment, environmentPolicy, verifyRuntime, verifyPreflight } from './evaluate-agent-workflow.mjs';
import {createHash} from 'node:crypto';

function removeFixture(root) {
  const resolved = fs.realpathSync(root);
  assert.equal(path.dirname(resolved), fs.realpathSync(os.tmpdir()));
  assert.ok(path.basename(resolved).startsWith('revenant-eval-'));
  fs.rmSync(resolved, { recursive: true, force: true });
}

const sha256 = value => createHash('sha256').update(value).digest('hex');
function isolatedRuntime(root) {
  const toolchain=path.join(root,'toolchain');fs.mkdirSync(toolchain);
  const packageJson=path.join(toolchain,'node_modules/@openai/codex/package.json'),platformJson=path.join(toolchain,'node_modules/@openai/codex-win32-x64/package.json'),lockFile=path.join(toolchain,'package-lock.json');
  fs.mkdirSync(path.dirname(packageJson),{recursive:true});fs.mkdirSync(path.dirname(platformJson),{recursive:true});
  const packageInfo={name:'@openai/codex',version:'0.159.1',integrity:'sha512-test',packageJson,lockFile,platform:{name:'@openai/codex-win32-x64',version:'0.159.1-win32-x64',integrity:'sha512-platform',packageJson:platformJson}};
  fs.writeFileSync(packageJson,JSON.stringify({name:packageInfo.name,version:packageInfo.version}));
  fs.writeFileSync(platformJson,JSON.stringify({name:packageInfo.platform.name,version:packageInfo.platform.version}));
  fs.writeFileSync(lockFile,JSON.stringify({packages:{'node_modules/@openai/codex':{version:packageInfo.version,integrity:packageInfo.integrity},'node_modules/@openai/codex-win32-x64':{version:packageInfo.platform.version,integrity:packageInfo.platform.integrity}}}));
  const files=['cli','codeModeHost','commandRunner','sandboxSetup','node'].map(role=>{
    const vendor='node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc';
    const layout={cli:`${vendor}/bin/codex.exe`,codeModeHost:`${vendor}/bin/codex-code-mode-host.exe`,commandRunner:`${vendor}/codex-resources/codex-command-runner.exe`,sandboxSetup:`${vendor}/codex-resources/codex-windows-sandbox-setup.exe`,node:'node/node.exe'};
    const file=path.join(toolchain,layout[role]);fs.mkdirSync(path.dirname(file),{recursive:true});
    if(role==='cli'||role==='node')fs.copyFileSync(process.execPath,file);else fs.writeFileSync(file,role);
    return {role,path:file,sha256:sha256(fs.readFileSync(file))};
  });
  const runtime={schemaVersion:1,root:toolchain,package:packageInfo,files};
  return {runtime,command:files.find(file=>file.role==='cli').path};
}
function successfulPreflight(config,root) {
  const workspace=path.join(root,'preflight-workspace');fs.mkdirSync(workspace,{recursive:true});
  const proof=path.join(workspace,'patch-proof.txt');fs.writeFileSync(proof,'created then updated');
  const events=[{type:'item.completed',item:{id:'command',type:'command_execution',status:'completed',exit_code:0}},{type:'item.completed',item:{id:'add',type:'file_change',status:'completed',changes:[{path:proof,kind:'add'}]}},{type:'item.completed',item:{id:'update',type:'file_change',status:'completed',changes:[{path:proof,kind:'update'}]}},{type:'turn.completed'}];
  const execution={exitCode:0,timedOut:false,processError:null,stdout:{truncated:false},stderr:{truncated:false},parseTruncated:false};
  const dependencies={schemaVersion:1,executions:['npm-version','cargo-version','npm-cache','cargo-cache'].map(id=>({id,command:'mock-command',args:id.endsWith('cache')?['--offline']:['--version'],exitCode:0,timedOut:false,processError:null}))};
  const records=Object.fromEntries(Object.entries({events:events.map(event=>JSON.stringify(event)).join('\n')+'\n',execution:JSON.stringify(execution),dependencies:JSON.stringify(dependencies)}).map(([role,contents])=>{
    const file=path.join(root,`preflight-${role}.json`);fs.writeFileSync(file,contents);return [role,{file,sha256:sha256(contents)}];
  }));
  const evidence={schemaVersion:1,passed:true,workspace,runtimeSha256:sha256(JSON.stringify(config.runtime)),environmentPolicySha256:sha256(JSON.stringify(environmentPolicy(config))),permissions:{profile:':workspace',windowsSandbox:'elevated',managedRules:true,ignoreUserConfig:true,delegation:false},checks:{command:true,patchCreate:true,patchUpdate:true,bytesVerified:true,dependencies:true},proof:{file:proof,sha256:sha256(fs.readFileSync(proof)),expectedText:'created then updated'},records};
  const file=path.join(root,'preflight.json');fs.writeFileSync(file,JSON.stringify(evidence));
  config.preflight={file,sha256:sha256(fs.readFileSync(file))};return evidence;
}

test('isolated child environment removes desktop state without altering tools, authentication or parent',()=>{
  const parent={PATH:'ordinary-tools',CODEX_HOME:'existing-auth-home',OPENAI_API_KEY:'test-placeholder',CODEX_API_KEY:'test-placeholder',CODEX_APP_TOOLS_PIPE_PATH:'desktop-pipe',CODEX_SESSION_ID:'desktop-session',CODEX_CLI_PATH:'desktop-cli',CODEX_PERMISSION_PROFILE:'desktop-profile',CODEX_MCP_NODE_PATH:'desktop-node',CODEX_INTERNAL_ORIGINATOR_OVERRIDE:'desktop'};
  const original={...parent},config={runtime:{package:{packageJson:path.resolve('private/package.json')},files:[{role:'node',path:path.resolve('private/node.exe')}]}};
  const child=buildChildEnvironment(config,parent);
  assert.deepEqual(parent,original);
  assert.equal(child.PATH.split(path.delimiter)[0],path.resolve('private'));assert.ok(child.PATH.split(path.delimiter).includes(parent.PATH));assert.equal(child.CODEX_HOME,parent.CODEX_HOME);assert.equal(child.OPENAI_API_KEY,parent.OPENAI_API_KEY);assert.equal(child.CODEX_API_KEY,parent.CODEX_API_KEY);
  for(const name of ['CODEX_APP_TOOLS_PIPE_PATH','CODEX_SESSION_ID','CODEX_CLI_PATH','CODEX_PERMISSION_PROFILE','CODEX_INTERNAL_ORIGINATOR_OVERRIDE'])assert.equal(name in child,false);
  assert.equal(child.CODEX_MCP_NODE_PATH,path.resolve('private/node.exe'));
  assert.equal(child.CODEX_MANAGED_PACKAGE_ROOT,path.resolve('private'));
});

test('runtime rejects missing companions even when main CLI hash still matches, and rejects drift',()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'revenant-eval-runtime-'));
  try {
    const fixture=isolatedRuntime(root),config={allowedRoot:root,codex:{command:fixture.command},runtime:fixture.runtime};
    assert.equal(verifyRuntime(config).schemaVersion,1);
    const host=fixture.runtime.files.find(file=>file.role==='codeModeHost');fs.unlinkSync(host.path);
    assert.throws(()=>verifyRuntime(config),/codeModeHost.*missing/i);
    fs.writeFileSync(host.path,'codeModeHost');
    const runner=fixture.runtime.files.find(file=>file.role==='commandRunner');fs.writeFileSync(runner.path,'changed runner');
    assert.throws(()=>verifyRuntime(config),/commandRunner.*changed/i);
  } finally {removeFixture(root);}
});

test('runtime rejects relocated or swapped companions even with refreshed matching hashes',()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'revenant-eval-layout-'));
  try {
    const fixture=isolatedRuntime(root),config={allowedRoot:root,codex:{command:fixture.command},runtime:fixture.runtime};
    const host=fixture.runtime.files.find(file=>file.role==='codeModeHost'),original=host.path,relocated=path.join(fixture.runtime.root,'relocated-host.exe');
    fs.copyFileSync(host.path,relocated);host.path=relocated;
    assert.throws(()=>verifyRuntime(config),/codeModeHost.*layout/i);
    host.path=original;
    const runner=fixture.runtime.files.find(file=>file.role==='commandRunner'),setup=fixture.runtime.files.find(file=>file.role==='sandboxSetup');
    [runner.path,setup.path]=[setup.path,runner.path];[runner.sha256,setup.sha256]=[setup.sha256,runner.sha256];
    assert.throws(()=>verifyRuntime(config),/commandRunner.*layout/i);
  } finally {removeFixture(root);}
});

test('preflight rejects claimed successful flags without successful execution, patch events and dependency records',()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'revenant-eval-records-'));
  try {
    const fixture=isolatedRuntime(root),config={allowedRoot:root,codex:{command:fixture.command},runtime:fixture.runtime,permissionsProfile:':workspace',windowsSandbox:'elevated'};
    const freeze=evidence=>{fs.writeFileSync(config.preflight.file,JSON.stringify(evidence));config.preflight.sha256=sha256(fs.readFileSync(config.preflight.file));};
    let evidence=successfulPreflight(config,root);
    fs.writeFileSync(evidence.records.events.file,JSON.stringify({type:'turn.completed'})+'\n');evidence.records.events.sha256=sha256(fs.readFileSync(evidence.records.events.file));freeze(evidence);
    assert.throws(()=>verifyPreflight(config),/successful command/i);
    evidence=successfulPreflight(config,root);
    const events=fs.readFileSync(evidence.records.events.file,'utf8').trim().split('\n').map(line=>JSON.parse(line));events[2].item.status='failed';
    fs.writeFileSync(evidence.records.events.file,events.map(event=>JSON.stringify(event)).join('\n'));evidence.records.events.sha256=sha256(fs.readFileSync(evidence.records.events.file));freeze(evidence);
    assert.throws(()=>verifyPreflight(config),/distinct.*patch/i);
    evidence=successfulPreflight(config,root);
    const execution=JSON.parse(fs.readFileSync(evidence.records.execution.file));execution.exitCode=1;
    fs.writeFileSync(evidence.records.execution.file,JSON.stringify(execution));evidence.records.execution.sha256=sha256(fs.readFileSync(evidence.records.execution.file));freeze(evidence);
    assert.throws(()=>verifyPreflight(config),/actual model execution/i);
    evidence=successfulPreflight(config,root);
    const dependencies=JSON.parse(fs.readFileSync(evidence.records.dependencies.file));dependencies.executions[3].exitCode=1;
    fs.writeFileSync(evidence.records.dependencies.file,JSON.stringify(dependencies));evidence.records.dependencies.sha256=sha256(fs.readFileSync(evidence.records.dependencies.file));freeze(evidence);
    assert.throws(()=>verifyPreflight(config),/cargo-cache/i);
    evidence=successfulPreflight(config,root);evidence.proof.expectedText='invented final contents';freeze(evidence);
    assert.throws(()=>verifyPreflight(config),/expected final bytes/i);
  } finally {removeFixture(root);}
});

test('frozen preflight rejects failed patch verification and later proof-byte drift',()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'revenant-eval-preflight-'));
  try {
    const fixture=isolatedRuntime(root),config={allowedRoot:root,codex:{command:fixture.command},runtime:fixture.runtime,permissionsProfile:':workspace',windowsSandbox:'elevated'};
    const evidence=successfulPreflight(config,root);assert.equal(verifyPreflight(config).passed,true);
    evidence.checks.patchUpdate=false;fs.writeFileSync(config.preflight.file,JSON.stringify(evidence));config.preflight.sha256=sha256(fs.readFileSync(config.preflight.file));
    assert.throws(()=>verifyPreflight(config),/patchUpdate/);
    evidence.checks.patchUpdate=true;fs.writeFileSync(config.preflight.file,JSON.stringify(evidence));config.preflight.sha256=sha256(fs.readFileSync(config.preflight.file));fs.writeFileSync(evidence.proof.file,'claimed-success');
    assert.throws(()=>verifyPreflight(config),/patch proof/i);
  } finally {removeFixture(root);}
});

test('failed preflight or runtime drift stops before CLI requests, output or fixture mutations',async()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'revenant-eval-gate-'));
  try {
    const fixture=isolatedRuntime(root),marker=path.join(root,'unexpected-work'),program=`require('node:fs').writeFileSync(${JSON.stringify(marker)},'launched')`;
    const cli=path.join(root,'cli.mjs');fs.writeFileSync(cli,`import fs from 'node:fs';fs.writeFileSync(${JSON.stringify(marker)},'CLI launched');`);
    const cases=fixedCases.map(spec=>{
      const workspace=path.join(root,spec.id);fs.mkdirSync(workspace);
      return {id:spec.id,workspace,preparation:[{command:process.execPath,args:['--eval',program]}],verifiers:spec.requiredEvidence.map(role=>({role,command:process.execPath,args:['--eval',program],assertions:[{path:'version',equals:4}],agentOperation:'test.operation'})),mcp:spec.id==='scaffold'?undefined:{command:process.execPath,args:[]},eventAssertions:spec.id==='native-failure'?[{tool:'test.operation',assertions:[{path:'code',equals:'failure'}]}]:undefined,expectedMcpTool:spec.id==='packaged-mcp'?'test.operation':undefined,expectedMcpArguments:spec.id==='packaged-mcp'?{}:undefined};
    });
    const config={model:'gpt-6.1-sol',reasoningEffort:'xhigh',permissionsProfile:':workspace',windowsSandbox:'elevated',codex:{command:fixture.command,prefix:[cli]},runtime:fixture.runtime,allowedRoot:root,output:path.join(root,'results'),timeoutMs:5000,maxLogBytes:4096,cases};
    const evidence=successfulPreflight(config,root);evidence.checks.bytesVerified=false;
    fs.writeFileSync(config.preflight.file,JSON.stringify(evidence));config.preflight.sha256=sha256(fs.readFileSync(config.preflight.file));
    await assert.rejects(runEvaluation(config),/bytesVerified/);
    assert.equal(fs.existsSync(marker),false);assert.equal(fs.existsSync(config.output),false);
    successfulPreflight(config,root);
    fs.writeFileSync(fixture.runtime.files.find(file=>file.role==='sandboxSetup').path,'changed setup');
    await assert.rejects(runEvaluation(config),/sandboxSetup.*changed/);
    assert.equal(fs.existsSync(marker),false);assert.equal(fs.existsSync(config.output),false);
  } finally {removeFixture(root);}
});

test('exec JSON usage sums completed turns and counts tool items once', () => {
  const metrics = createEventMetrics();
  for (const event of [
    { type: 'item.started', item: { id: '1', type: 'command_execution', command: 'inspect' } },
    { type: 'item.completed', item: { id: '1', type: 'command_execution', exit_code: 0 } },
    { type: 'item.started', item: { id: '2', type: 'mcp_tool_call', server: 'revenant', tool: 'catalog' } },
    { type: 'item.completed', item: { id: '2', type: 'mcp_tool_call', status: 'completed' } },
    { type: 'turn.completed', usage: { input_tokens: 100, cached_input_tokens: 40, output_tokens: 20 } },
    { type: 'turn.completed', usage: { input_tokens: 50, cached_input_tokens: 0, output_tokens: 10 } },
  ]) metrics.accept(event);
  assert.deepEqual(metrics.snapshot().tokens, { input: 150, cachedInput: 40, output: 30, total: 180 });
  assert.equal(metrics.snapshot().toolCalls.command, 1);
  assert.equal(metrics.snapshot().toolCalls.mcp, 1);
  assert.equal(metrics.snapshot().toolCalls.total, 2);
  assert.equal(metrics.snapshot().completedTurns, 2);
});

test('a failed turn cannot become completed because an agent reports success', () => {
  const metrics = createEventMetrics();
  metrics.accept({ type: 'item.completed', item: { id: 'answer', type: 'agent_message', text: 'Everything passed.' } });
  metrics.accept({ type: 'turn.failed', error: { message: 'quota exhausted' } });
  const result = summarizeRun({ metrics: metrics.snapshot(), exitCode: 1, elapsedMs: 123, verifications: [{ passed: true }], interventionCount: 0 });
  assert.equal(result.completed, false);
  assert.equal(result.agentCompleted, false);
  assert.equal(result.tokens, null);
  assert.equal(result.usageStatus, 'unavailable');
  assert.equal(result.elapsedMs, 123);
  assert.equal(result.interventions, 0);
  assert.deepEqual(result.failures, ['quota exhausted']);
});

test('incomplete usage remains partial without manufacturing zero tokens', () => {
  const metrics = createEventMetrics();
  metrics.accept({ type: 'turn.completed', usage: { input_tokens: 8, cached_input_tokens: 2, output_tokens: 4 } });
  metrics.accept({ type: 'turn.completed' });
  assert.equal(metrics.snapshot().usageStatus, 'partial');
  assert.deepEqual(metrics.snapshot().tokens, { input: 8, cachedInput: 2, output: 4, total: 12 });
});

test('independent verifier failure prevents workflow completion', () => {
  const metrics = createEventMetrics();
  metrics.accept({ type: 'turn.completed', usage: { input_tokens: 1, cached_input_tokens: 0, output_tokens: 1 } });
  const result = summarizeRun({ metrics: metrics.snapshot(), exitCode: 0, elapsedMs: 1, verifications: [{ passed: false }], interventionCount: 2 });
  assert.equal(result.agentCompleted, true);
  assert.equal(result.completed, false);
  assert.equal(result.interventions, 2);
});

test('fixed configuration requires every predefined case and separate verifier evidence', () => {
  assert.equal(fixedCases.length, 5);
  assert.throws(() => validateConfiguration({ model: 'other', reasoningEffort: 'xhigh' }), /gpt-6.1-sol/);
  assert.throws(() => validateConfiguration({ model: 'gpt-6.1-sol', reasoningEffort: 'xhigh', cases: [] }), /five/);
});

test('agent arguments select the tested named workspace profile without overriding it with legacy sandbox flags', () => {
  const args = agentArguments({ model: 'gpt-6.1-sol', reasoningEffort: 'xhigh', permissionsProfile: ':workspace', windowsSandbox: 'elevated', codex: { prefix: [] } }, { workspace: 'isolated-workspace' });
  assert.ok(args.includes('default_permissions=":workspace"'));
  assert.ok(args.includes('windows.sandbox="elevated"'));
  assert.ok(args.includes('--ignore-user-config'));
  assert.ok(args.includes('multi_agent'));
  assert.ok(args.includes('multi_agent_v2'));
  assert.equal(args.includes('--sandbox'), false);
  assert.equal(args.some(arg => /danger|ignore-rules|bypass/.test(arg)), false);
  assert.throws(() => agentArguments({ permissionsProfile: ':danger-full-access' }, { workspace: 'isolated-workspace' }), /:workspace/);
  assert.throws(() => agentArguments({ permissionsProfile: ':workspace', windowsSandbox: 'unelevated' }, { workspace: 'isolated-workspace' }), /prepared elevated/);
});

test('failed later turn marks earlier recorded usage partial', () => {
  const metrics = createEventMetrics();
  metrics.accept({ type: 'turn.completed', usage: { input_tokens: 8, cached_input_tokens: 2, output_tokens: 4 } });
  metrics.accept({ type: 'turn.failed', error: { message: 'interrupted' } });
  assert.equal(metrics.snapshot().usageStatus, 'partial');
});

test('bounded subprocess logs preserve streaming JSON across split UTF-8 and count uncaptured events', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'revenant-eval-test-'));
  const lines = [];
  try {
    const program = `const b=Buffer.from(JSON.stringify({type:'message',text:'日本語'})+'\\n'); const p=b.indexOf(Buffer.from('日本語'))+1; process.stdout.write(b.subarray(0,p)); setTimeout(()=>{process.stdout.write(b.subarray(p)); process.stdout.write(JSON.stringify({type:'turn.completed',usage:{input_tokens:9,cached_input_tokens:0,output_tokens:3}})+'\\n');},10);`;
    const result = await execute(process.execPath, ['--eval', program], { cwd: root, timeoutMs: 5000, maxLogBytes: 8, stdoutPath: path.join(root, 'out'), stderrPath: path.join(root, 'err'), onLine(line) { lines.push(JSON.parse(line)); } });
    assert.equal(result.exitCode, 0);
    assert.equal(fs.statSync(path.join(root, 'out')).size, 8);
    assert.equal(result.stdout.truncated, true);
    assert.equal(lines[0].text, '日本語');
    assert.equal(lines[1].usage.input_tokens, 9);
  } finally { removeFixture(root); }
});

test('independent compiled manifest rejects obsolete protocol and non-exposed operations', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'revenant-eval-manifest-'));
  try {
    const verifier = { role: 'manifest', command: process.execPath, args: ['--eval', 'console.log(JSON.stringify({version:4,protocol:2,operations:[{id:"luma.workflowSummary",agent:false}]}))'], agentOperation: 'luma.workflowSummary', assertions: [{ path: 'version', equals: 4 }] };
    const checks = await verifyCase({ timeoutMs: 5000, maxLogBytes: 4096 }, { id: 'luma-feature', workspace: root, verifiers: [verifier] }, root, { mcpResults: [] });
    assert.equal(checks[0].passed, false);
    assert.equal(checks[0].manifest.passed, false);
    assert.equal(checks[0].manifest.protocol, 2);
    assert.equal(checks[0].manifest.agentExposed, false);
  } finally { removeFixture(root); }
});

test('structured native failure requires the real MCP result, not a claimed diagnosis', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'revenant-eval-error-'));
  try {
    const item = { id: 'native-failure', workspace: root, verifiers: [], eventAssertions: [{ tool: 'task.status', assertions: [{ path: 'error.code', equals: 'controlled_failure' }] }] };
    const checks = await verifyCase({}, item, root, { mcpResults: [{ server: 'revenant', tool: 'task.status', status: 'completed', result: { structured_content: { error: { code: 'other_failure' } } } }] });
    assert.equal(checks.length, 1);
    assert.equal(checks[0].passed, false);
    assert.equal(checks[0].role, 'mcp-event:task.status');
  } finally { removeFixture(root); }
});

test('a predeclared fixture setup failure remains an explicit failed prerequisite', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'revenant-eval-setup-'));
  try {
    const item = { workspace: root, preparation: [{ command: process.execPath, args: ['--eval', 'process.stderr.write("fixture drift");process.exitCode=1'] }] };
    const results = await prepareCase({ timeoutMs: 5000, maxLogBytes: 4096 }, item, root);
    assert.equal(results.length, 1);
    assert.equal(results[0].passed, false);
    assert.equal(results[0].exitCode, 1);
  } finally { removeFixture(root); }
});

test('all observed exec tool item types are counted and missing IDs mark counts partial', () => {
  const metrics = createEventMetrics();
  for (const [index, type] of ['command_execution', 'mcp_tool_call', 'file_change', 'web_search', 'collab_tool_call', 'todo_list'].entries()) {
    metrics.accept({type:'item.completed',item:{id:String(index),type}});
    metrics.accept({type:'item.updated',item:{id:String(index),type}});
  }
  metrics.accept({type:'item.completed',item:{type:'file_change'}});
  assert.equal(metrics.snapshot().toolCalls.total, 6);
  assert.equal(metrics.snapshot().toolCalls.fileChange, 1);
  assert.equal(metrics.snapshot().toolCalls.webSearch, 1);
  assert.equal(metrics.snapshot().toolCalls.collaboration, 1);
  assert.equal(metrics.snapshot().toolCallStatus, 'partial');
  assert.equal(metrics.snapshot().missingToolIds, 1);
});

test('timeout terminates only the owned process tree and reports cleanup evidence', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'revenant-eval-timeout-'));
  try {
    const pidFile = path.join(root,'descendant.pid');
    const program = `const {spawn}=require('node:child_process');const fs=require('node:fs');const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'});fs.writeFileSync(${JSON.stringify(pidFile)},String(child.pid));setInterval(()=>{},1000);`;
    const result = await execute(process.execPath,['--eval',program],{cwd:root,timeoutMs:500,maxLogBytes:4096,stdoutPath:path.join(root,'out'),stderrPath:path.join(root,'err')});
    assert.equal(result.timedOut,true);
    assert.equal(result.cleanup.status,'terminated_owned_tree');
    const descendant = Number(fs.readFileSync(pidFile,'utf8'));
    assert.throws(()=>process.kill(descendant,0),/ESRCH/);
  } finally { removeFixture(root); }
});

test('a changed verifier source rejects frozen provenance before an agent runs', () => {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'revenant-eval-provenance-'));
  try {
    const helper=path.join(root,'helper.mjs'),provenance=path.join(root,'provenance.json');
    fs.writeFileSync(helper,'original verifier');
    fs.writeFileSync(provenance,JSON.stringify({files:[{path:helper,sha256:createHash('sha256').update('original verifier').digest('hex')}]}));
    const config={provenance:{file:provenance,sha256:createHash('sha256').update(fs.readFileSync(provenance)).digest('hex')}};
    fs.writeFileSync(helper,'changed verifier');
    assert.throws(()=>verifyProvenance(config),/Frozen artifact changed/);
  } finally {removeFixture(root);}
});

test('an actual account usage-limit event stops before verifiers, fixture mutations and later agents', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'revenant-eval-quota-'));
  try {
    const cli = path.join(root, 'cli.mjs');
    fs.writeFileSync(cli, `import fs from 'node:fs';import path from 'node:path';
      if(process.argv.includes('--version'))console.log('codex-cli 0.159.1');
      else{fs.appendFileSync(path.join(${JSON.stringify(root)},'agent-invocations'),'1');
        const message='You’ve hit your usage limit. Try again later.';
        console.log(JSON.stringify({type:'error',message}));
        console.log(JSON.stringify({type:'turn.failed',error:{message}}));process.exitCode=1;}`);
    const markerProgram = `require('node:fs').appendFileSync(${JSON.stringify(path.join(root, 'unexpected-work'))},'1')`;
    const cases = fixedCases.map(spec => {
      const workspace = path.join(root, spec.id); fs.mkdirSync(workspace);
      return { id: spec.id, workspace,
        preparation: spec.id === 'scaffold' ? [] : [{ command: process.execPath, args: ['--eval', markerProgram] }],
        verifiers: spec.requiredEvidence.map(role => ({ role, command: process.execPath, args: ['--eval', markerProgram], assertions: [{ path: 'version', equals: 4 }], agentOperation: 'test.operation' })),
        mcp: spec.id === 'scaffold' ? undefined : { command: process.execPath, args: [] },
        eventAssertions: spec.id === 'native-failure' ? [{ tool: 'test.operation', assertions: [{ path: 'code', equals: 'failure' }] }] : undefined,
        expectedMcpTool: spec.id === 'packaged-mcp' ? 'test.operation' : undefined,
        expectedMcpArguments: spec.id === 'packaged-mcp' ? {} : undefined,
      };
    });
    const fixture=isolatedRuntime(root);
    const config = { model: 'gpt-6.1-sol', reasoningEffort: 'xhigh', permissionsProfile: ':workspace', windowsSandbox: 'elevated', codex: { command: fixture.command, prefix: [cli] }, runtime:fixture.runtime, allowedRoot: root, output: path.join(root, 'results'), timeoutMs: 5000, maxLogBytes: 4096, cases };
    successfulPreflight(config,root);
    const result = await runEvaluation(config);
    assert.equal(result.results.length, 1);
    assert.equal(result.results[0].id, 'scaffold');
    assert.equal(result.results[0].agentCompleted, false);
    assert.equal(result.results[0].tokens, null);
    assert.equal(result.results[0].usageStatus, 'unavailable');
    assert.deepEqual(result.results[0].verifications, []);
    assert.equal(fs.readFileSync(path.join(root, 'agent-invocations'), 'utf8'), '1');
    assert.equal(fs.existsSync(path.join(root, 'unexpected-work')), false);
    const aborted = JSON.parse(fs.readFileSync(path.join(config.output, 'aborted.json'), 'utf8'));
    assert.equal(aborted.reason, 'Account usage limit; evaluation stops before subsequent cases');
    assert.deepEqual(aborted.unattemptedCases, fixedCases.slice(1).map(spec => spec.id));
  } finally { removeFixture(root); }
});
