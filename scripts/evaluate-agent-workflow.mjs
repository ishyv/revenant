// Five local product workflows. Supply isolated, prepared roots; this runner never
// copies a live checkout, deletes projects, or starts app instances. Optional
// predeclared setup commands prepare isolated faults between sequential cases.
// Usage: node scripts/evaluate-agent-workflow.mjs --config <json> [--prepare]
import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import { StringDecoder } from 'node:string_decoder';

export const fixedCases = Object.freeze([
  { id: 'scaffold', requiredEvidence: ['rust', 'manifest', 'consumer', 'scaffold'], prompt: 'Use Revenant to scaffold an application in the provided empty test workspace. Enable its agent interface. Build its native operations and generate and check the Svelte consumer. Discover the supported commands and authoring model through the local documentation and Revenant MCP catalog first.' },
  { id: 'luma-feature', requiredEvidence: ['rust', 'manifest', 'consumer', 'feature'], prompt: 'First inspect and explain Luma\'s existing operation catalog and its catalog/UI architecture using Revenant inspection tools. Then add an annotated Rust operation luma.workflowSummary, read-only, independent, and exposed to agents. It must return the existing catalog image count without changing the catalog. Add a Svelte interface displaying that count using the generated useOperations callable. Regenerate the contract and validate Rust and the Svelte consumer.' },
  { id: 'contract-repair', requiredEvidence: ['rust', 'manifest', 'consumer', 'repair'], prompt: 'Diagnose the intentionally changed native operation contract in this isolated Luma workspace using the compiled catalog and consumer diagnostics. Repair the Svelte caller for the changed contract, regenerate bindings, and validate Rust and the consumer. Preserve the intentional native contract change. Explain the contract mismatch with source evidence.' },
  { id: 'native-failure', requiredEvidence: ['diagnosis'], prompt: 'Diagnose the deliberately failing native operation in this isolated Luma workspace. Use Revenant inspection and invocation tools to reproduce the failure and identify its authored Rust source. Save diagnosis.json containing operation, code, stage, sourceFile, and cause supported by the actual structured error. Do not fix or mask the injected failure.' },
  { id: 'packaged-mcp', requiredEvidence: ['live-result'], prompt: 'Use the configured Revenant MCP server for the already running packaged Luma instance. Inspect its agent-exposed catalog, then invoke the configured author-exposed read-only operation with the configured arguments. Report the actual structured result and instance identity. Save live-result.json containing operation, instanceId, and result. Do not start another app or change application data.' },
]);

export function createEventMetrics() {
  const names = {command_execution:'command',mcp_tool_call:'mcp',file_change:'fileChange',web_search:'webSearch',collab_tool_call:'collaboration',todo_list:'plan'};
  const counts = Object.fromEntries(Object.values(names).map(name => [name,new Set()]));
  let completedTurns = 0, usageTurns = 0, malformedEvents = 0, missingToolIds = 0;
  const tokens = { input: 0, cachedInput: 0, output: 0, total: 0 };
  const failures = [], mcpResults = [];
  return {
    accept(event) {
      if (!event || typeof event.type !== 'string') { malformedEvents++; return; }
      const item = event.item;
      if (item && ['item.started', 'item.updated', 'item.completed'].includes(event.type)) {
        const set = counts[names[item.type]];
        if (set) { if (typeof item.id === 'string') set.add(item.id); else missingToolIds++; }
        if (event.type === 'item.completed' && item.type === 'mcp_tool_call') mcpResults.push(item);
      }
      if (event.type === 'turn.completed') {
        completedTurns++;
        const usage = event.usage;
        if (usage && ['input_tokens', 'cached_input_tokens', 'output_tokens'].every(key => Number.isSafeInteger(usage[key]) && usage[key] >= 0)) {
          usageTurns++;
          tokens.input += usage.input_tokens; tokens.cachedInput += usage.cached_input_tokens;
          tokens.output += usage.output_tokens; tokens.total += usage.input_tokens + usage.output_tokens;
        }
      }
      if (event.type === 'turn.failed' || event.type === 'error') failures.push(event.error?.message ?? event.message ?? event.type);
    },
    malformed() { malformedEvents++; },
    snapshot() {
      const toolCalls=Object.fromEntries(Object.entries(counts).map(([name,items])=>[name,items.size]));
      toolCalls.total=Object.values(toolCalls).reduce((sum,value)=>sum+value,0);
      return { completedTurns, tokens: usageTurns ? { ...tokens } : null, usageStatus: !usageTurns ? 'unavailable' : usageTurns === completedTurns && failures.length === 0 ? 'complete' : 'partial', toolCalls, toolCallStatus:missingToolIds||malformedEvents?'partial':'complete', toolCallScope:'Observed exec item IDs, including file changes, web searches, collaboration and plan updates; parent turn usage only', missingToolIds, malformedEvents, failures: [...failures], mcpResults: [...mcpResults] };
    },
  };
}

export function summarizeRun({ metrics, exitCode, elapsedMs, verifications, interventionCount, timedOut = false }) {
  const agentCompleted = exitCode === 0 && !timedOut && metrics.completedTurns > 0 && metrics.failures.length === 0 && metrics.malformedEvents === 0;
  return { ...metrics, exitCode, elapsedMs, timedOut, interventions: interventionCount, agentCompleted, completed: agentCompleted && verifications.length > 0 && verifications.every(item => item.passed), verifications };
}

function within(root, target) {
  const relative = path.relative(root, target);
  return relative !== '' && relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
}
function assertContained(root, target) {
  const actualRoot = fs.realpathSync(root), actual = fs.realpathSync(target);
  if (!within(actualRoot, actual)) throw new Error(`Case workspace must be a strict child of allowedRoot: ${target}`);
}

function validatePermissions(config) {
  if (config.permissionsProfile !== ':workspace') throw new Error('Fixed permissionsProfile must be :workspace');
  if (config.windowsSandbox !== undefined && config.windowsSandbox !== 'elevated') throw new Error('Windows evaluation requires the prepared elevated sandbox');
  if (process.platform === 'win32' && config.windowsSandbox !== 'elevated') throw new Error('Windows evaluation must explicitly select the prepared elevated sandbox');
}

const runtimeRoles=Object.freeze(['cli','codeModeHost','commandRunner','sandboxSetup','node']);
const preservedCodexEnvironment=Object.freeze(['CODEX_HOME','CODEX_CI','CODEX_API_KEY']);
const fileDigest=file=>createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const recordDigest=value=>createHash('sha256').update(JSON.stringify(value)).digest('hex');
const samePath=(first,second)=>path.relative(first,second)==='';
const windowsVendor='node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc';
const windowsRuntimeLayout=Object.freeze({cli:`${windowsVendor}/bin/codex.exe`,codeModeHost:`${windowsVendor}/bin/codex-code-mode-host.exe`,commandRunner:`${windowsVendor}/codex-resources/codex-command-runner.exe`,sandboxSetup:`${windowsVendor}/codex-resources/codex-windows-sandbox-setup.exe`,node:'node/node.exe'});

// Record policy and selected paths, never inherited environment values or secrets.
export function environmentPolicy(config) {
  const node=config.runtime?.files?.find(file=>file.role==='node')?.path,cli=config.runtime?.files?.find(file=>file.role==='cli')?.path;
  return {schemaVersion:1,removePrefix:'CODEX_',preserve:[...preservedCodexEnvironment],path:{prepend:[node&&path.dirname(node),cli&&path.dirname(cli),cli&&path.resolve(path.dirname(cli),'../codex-path')].filter(Boolean),remove:'Inherited CLI/Node runtime directories, desktop WindowsApps Codex bins, and .codex/tmp/arg0 shims'},set:{CODEX_MCP_NODE_PATH:node,CODEX_MANAGED_PACKAGE_ROOT:path.dirname(config.runtime?.package?.packageJson??''),CODEX_MANAGED_BY_NPM:'1'}};
}

export function buildChildEnvironment(config,parent=process.env) {
  const policy=environmentPolicy(config),child={...parent};
  if(!policy.set.CODEX_MCP_NODE_PATH)throw new Error('Isolated runtime requires a private Node executable');
  for(const name of Object.keys(child))if(name.toUpperCase().startsWith(policy.removePrefix)&&!policy.preserve.includes(name.toUpperCase()))delete child[name];
  const pathKey=Object.keys(parent).find(name=>name.toUpperCase()==='PATH');
  const inheritedRuntimeDirectories=['CODEX_CLI_PATH','CODEX_MCP_NODE_PATH'].map(name=>parent[name]&&path.dirname(parent[name]).toLowerCase()).filter(Boolean);
  const retained=(parent[pathKey]??'').split(path.delimiter).filter(entry=>entry&&!inheritedRuntimeDirectories.includes(entry.toLowerCase())&&!/[\\/]WindowsApps[\\/]OpenAI\.Codex[^\\/]*[\\/]/i.test(entry)&&!/[\\/]\.codex[\\/]tmp[\\/]arg0[\\/]/i.test(entry));
  for(const name of Object.keys(child))if(name.toUpperCase()==='PATH')delete child[name];
  child.PATH=[...new Set([...policy.path.prepend,...retained])].join(path.delimiter);
  return Object.assign(child,policy.set);
}

export function verifyRuntime(config) {
  const runtime=config.runtime;
  if(runtime?.schemaVersion!==1||!path.isAbsolute(runtime.root??''))throw new Error('Isolated runtime schemaVersion=1 and absolute root required');
  assertContained(config.allowedRoot,runtime.root);
  const containedFile=(file,role)=>{
    try {assertContained(runtime.root,file);if(!fs.statSync(file).isFile())throw new Error('not a file');}
    catch(error){throw new Error(`Runtime ${role} missing or outside private toolchain: ${error.message}`);}
  };
  const pkg=runtime.package;
  if(pkg?.name!=='@openai/codex'||pkg.version!=='0.159.1'||!pkg.integrity?.startsWith('sha512-'))throw new Error('Runtime requires pinned @openai/codex@0.159.1 and package integrity');
  containedFile(pkg.lockFile,'package lock');
  const exactLayout=(file,relative,role)=>{
    if(!samePath(path.resolve(file),path.resolve(runtime.root,relative))||!samePath(fs.realpathSync(file),path.resolve(fs.realpathSync(runtime.root),relative)))throw new Error(`Runtime ${role} layout must match the complete pinned package`);
  };
  exactLayout(pkg.lockFile,'package-lock.json','package lock');
  const lock=JSON.parse(fs.readFileSync(pkg.lockFile,'utf8'));
  const checkPackage=spec=>{
    containedFile(spec.packageJson,`${spec.name} metadata`);
    const metadata=JSON.parse(fs.readFileSync(spec.packageJson,'utf8')),entry=lock.packages?.[`node_modules/${spec.name}`];
    const expectedName=spec===pkg.platform?'@openai/codex':spec.name;
    if(![spec.name,expectedName].includes(metadata.name)||metadata.version!==spec.version||entry?.version!==spec.version||entry?.integrity!==spec.integrity)throw new Error(`Runtime package metadata or integrity mismatch: ${spec.name}`);
  };
  checkPackage(pkg);
  exactLayout(pkg.packageJson,'node_modules/@openai/codex/package.json','package metadata');
  if(process.platform==='win32'&&!pkg.platform)throw new Error('Windows runtime requires the complete platform package');
  if(pkg.platform){
    if(pkg.platform.name!=='@openai/codex-win32-x64'||pkg.platform.version!=='0.159.1-win32-x64'||!pkg.platform.integrity?.startsWith('sha512-'))throw new Error('Runtime Windows platform package must be pinned to 0.159.1-win32-x64');
    checkPackage(pkg.platform);
    exactLayout(pkg.platform.packageJson,'node_modules/@openai/codex-win32-x64/package.json','platform metadata');
  }
  if(!Array.isArray(runtime.files)||runtime.files.length!==runtimeRoles.length||new Set(runtime.files.map(file=>file.role)).size!==runtimeRoles.length)throw new Error('Runtime requires exactly five distinct companion roles');
  const actualPaths=new Set();
  for(const role of runtimeRoles){
    const file=runtime.files.find(file=>file.role===role);
    if(!file||!path.isAbsolute(file.path??'')||!/^[a-f0-9]{64}$/.test(file.sha256??''))throw new Error(`Runtime ${role} absolute path and SHA-256 required`);
    containedFile(file.path,role);
    if(process.platform==='win32'||pkg.platform)exactLayout(file.path,windowsRuntimeLayout[role],role);
    const actual=fs.realpathSync(file.path);
    if(actualPaths.has(actual))throw new Error(`Runtime ${role} must have its own companion file`);
    actualPaths.add(actual);
    if(fileDigest(file.path)!==file.sha256)throw new Error(`Runtime ${role} changed: ${file.path}`);
    if(role==='cli'&&actual!==fs.realpathSync(config.codex.command))throw new Error('Runtime CLI must match codex.command');
  }
  return runtime;
}

export function verifyPreflight(config) {
  const frozen=config.preflight;
  if(!frozen||!path.isAbsolute(frozen.file??'')||!/^[a-f0-9]{64}$/.test(frozen.sha256??''))throw new Error('Frozen successful isolated runtime preflight required');
  assertContained(config.allowedRoot,frozen.file);
  if(fileDigest(frozen.file)!==frozen.sha256)throw new Error('Frozen preflight record changed');
  const evidence=JSON.parse(fs.readFileSync(frozen.file,'utf8'));
  if(evidence.schemaVersion!==1||evidence.passed!==true)throw new Error('Isolated runtime preflight failed');
  if(evidence.runtimeSha256!==recordDigest(config.runtime)||evidence.environmentPolicySha256!==recordDigest(environmentPolicy(config)))throw new Error('Preflight runtime or environment policy does not match evaluation');
  const permissions={profile:config.permissionsProfile,windowsSandbox:config.windowsSandbox,managedRules:true,ignoreUserConfig:true,delegation:false};
  if(!isDeepStrictEqual(evidence.permissions,permissions))throw new Error('Preflight permissions do not match fixed evaluation boundaries');
  for(const check of ['command','patchCreate','patchUpdate','bytesVerified','dependencies'])if(evidence.checks?.[check]!==true)throw new Error(`Preflight check failed: ${check}`);
  if(!evidence.proof||!path.isAbsolute(evidence.proof.file??'')||!/^[a-f0-9]{64}$/.test(evidence.proof.sha256??''))throw new Error('Preflight independent patch proof required');
  assertContained(config.allowedRoot,evidence.proof.file);
  if(fileDigest(evidence.proof.file)!==evidence.proof.sha256)throw new Error('Preflight independent patch proof bytes changed');
  if(typeof evidence.proof.expectedText!=='string'||Buffer.byteLength(evidence.proof.expectedText)>64*1024||!fs.readFileSync(evidence.proof.file).equals(Buffer.from(evidence.proof.expectedText)))throw new Error('Preflight independent patch proof must match expected final bytes');
  if(!path.isAbsolute(evidence.workspace??''))throw new Error('Preflight model workspace required');
  assertContained(config.allowedRoot,evidence.workspace);
  const readRecord=(role,maxBytes)=>{
    const record=evidence.records?.[role];
    if(!record||!path.isAbsolute(record.file??'')||!/^[a-f0-9]{64}$/.test(record.sha256??''))throw new Error(`Preflight frozen ${role} record required`);
    assertContained(config.allowedRoot,record.file);
    if(fs.statSync(record.file).size>maxBytes||fileDigest(record.file)!==record.sha256)throw new Error(`Preflight ${role} record changed or exceeds bound`);
    return fs.readFileSync(record.file,'utf8');
  };
  const execution=JSON.parse(readRecord('execution',1024*1024));
  if(execution.exitCode!==0||execution.timedOut!==false||execution.processError!==null||execution.stdout?.truncated!==false||execution.stderr?.truncated!==false||execution.parseTruncated!==false)throw new Error('Preflight actual model execution did not succeed with complete logs');
  const events=readRecord('events',8*1024*1024).split(/\r?\n/).filter(line=>line.trim()).map(line=>JSON.parse(line));
  const successfulCommand=events.some(event=>event.type==='item.completed'&&event.item?.type==='command_execution'&&event.item.status==='completed'&&event.item.exit_code===0);
  if(!successfulCommand)throw new Error('Preflight actual events contain no successful command');
  const patchEvents=events.filter(event=>event.type==='item.completed'&&event.item?.type==='file_change'&&event.item.status==='completed'&&typeof event.item.id==='string').map(event=>event.item);
  const targets=change=>typeof change.path==='string'&&samePath(path.resolve(evidence.workspace,change.path),evidence.proof.file);
  const add=patchEvents.findIndex(item=>item.changes?.some(change=>targets(change)&&change.kind==='add'));
  const update=patchEvents.findIndex((item,index)=>index>add&&item.id!==patchEvents[add]?.id&&item.changes?.some(change=>targets(change)&&change.kind==='update'));
  if(add<0||update<0)throw new Error('Preflight requires distinct successful ordered add/update patch events for the proof file');
  if(!events.some(event=>event.type==='turn.completed')||events.some(event=>['error','turn.failed'].includes(event.type)))throw new Error('Preflight actual agent turn failed or did not complete');
  const dependencies=JSON.parse(readRecord('dependencies',1024*1024));
  const dependencyIds=['npm-version','cargo-version','npm-cache','cargo-cache'];
  if(dependencies.schemaVersion!==1||dependencies.executions?.length!==dependencyIds.length||new Set(dependencies.executions.map(item=>item.id)).size!==dependencyIds.length)throw new Error('Preflight requires all four actual dependency probe executions');
  for(const id of dependencyIds){
    const result=dependencies.executions.find(item=>item.id===id);
    if(!result||typeof result.command!=='string'||!result.command||!Array.isArray(result.args)||!result.args.every(arg=>typeof arg==='string')||result.exitCode!==0||result.timedOut!==false||result.processError!==null||(id.endsWith('-cache')&&!result.args.includes('--offline')))throw new Error(`Preflight dependency probe failed: ${id}`);
  }
  return evidence;
}

export function validateConfiguration(config) {
  if (config.model !== 'gpt-6.1-sol') throw new Error('Fixed model must be gpt-6.1-sol');
  if (config.reasoningEffort !== 'xhigh') throw new Error('Fixed reasoningEffort must be xhigh');
  if (!Array.isArray(config.cases) || config.cases.length !== 5 || new Set(config.cases.map(item => item.id)).size !== 5) throw new Error('Configure exactly the five fixed workflow cases');
  for (const spec of fixedCases) {
    const item = config.cases.find(item => item.id === spec.id);
    if (!item) throw new Error(`Missing fixed case ${spec.id}`);
    if (!path.isAbsolute(item.workspace ?? '')) throw new Error(`${spec.id}: workspace must be absolute`);
    if (!Array.isArray(item.verifiers)) throw new Error(`${spec.id}: independent verifiers required`);
    for (const role of spec.requiredEvidence) if (!item.verifiers.some(check => check.role === role)) throw new Error(`${spec.id}: missing independent ${role} verifier`);
    for (const check of item.verifiers) {
      if (!check.command || !Array.isArray(check.args)) throw new Error(`${spec.id}: verifiers require command and argument array`);
      if (['manifest', 'diagnosis', 'live-result'].includes(check.role) && (!Array.isArray(check.assertions) || check.assertions.length === 0)) throw new Error(`${spec.id}: ${check.role} requires structured JSON assertions`);
      if (check.role === 'manifest' && typeof check.agentOperation !== 'string') throw new Error(`${spec.id}: manifest verifier must name an agent-exposed operation`);
    }
    if (spec.id !== 'scaffold' && (!item.mcp?.command || !Array.isArray(item.mcp.args))) throw new Error(`${spec.id}: explicit MCP command and arguments required`);
    if (spec.id === 'native-failure' && !item.eventAssertions?.some(check => check.assertions?.length)) throw new Error('native-failure requires an assertion on the actual structured MCP error or failed task result');
    if (spec.id === 'packaged-mcp' && (!item.expectedMcpTool || !item.expectedMcpArguments)) throw new Error('packaged-mcp requires an exact expected tool and arguments');
  }
  if (!path.isAbsolute(config.allowedRoot ?? '') || !path.isAbsolute(config.output ?? '')) throw new Error('allowedRoot and output must be absolute');
  if (!config.codex?.command || !Array.isArray(config.codex.prefix ?? [])) throw new Error('codex.command executable required; prefix supports a Node CLI entrypoint');
  for (const key of ['timeoutMs', 'maxLogBytes']) if (!Number.isSafeInteger(config[key]) || config[key] < 1) throw new Error(`${key} must be a positive integer`);
  validatePermissions(config);
  for (const item of config.cases) assertContained(config.allowedRoot, item.workspace);
  return config;
}

function boundedAppend(fd, buffer, state, limit) {
  const remaining = Math.max(0, limit - state.bytes);
  const part = buffer.subarray(0, remaining);
  if (part.length) fs.writeSync(fd, part);
  state.bytes += part.length;
  if (buffer.length > remaining) state.truncated = true;
}

// No shell interpolation: callers supplying a .cmd shim should instead specify
// node.exe + the installed codex.js entrypoint, or the Codex native executable.
export async function execute(command, args, { cwd, timeoutMs, maxLogBytes, stdoutPath, stderrPath, onLine, input, env }) {
  const stdoutState = { bytes: 0, truncated: false }, stderrState = { bytes: 0, truncated: false };
  const out = fs.openSync(stdoutPath, 'wx'), err = fs.openSync(stderrPath, 'wx');
  let pending = '', dropping = false, parseTruncated = false, timedOut = false, processError = null;
  let cleanup = {status:'not_needed'}, cleanupPromise = Promise.resolve();
  const decoder = new StringDecoder('utf8');
  const start = performance.now();
  let timer;
  const result = await new Promise(resolve => {
    const child = spawn(command, args, { cwd, env, shell: false, windowsHide: true, detached:process.platform!=='win32', stdio: ['pipe', 'pipe', 'pipe'] });
    timer = setTimeout(() => {
      timedOut = true;
      const ownedPid=child.pid;
      if (!Number.isSafeInteger(ownedPid)) {cleanup={status:'unknown',reason:'No owned child PID'};return;}
      if(process.platform==='win32') cleanupPromise=new Promise(done=>{
        const killer=spawn(path.join(process.env.SystemRoot??'C:\\Windows','System32/taskkill.exe'),['/PID',String(ownedPid),'/T','/F'],{windowsHide:true,shell:false,stdio:'ignore'});
        killer.on('error',error=>{cleanup={status:'unknown',ownedPid,error:error.message};child.kill();done();});
        killer.on('close',code=>{cleanup={status:code===0?'terminated_owned_tree':'unknown',ownedPid,exitCode:code};if(code!==0)child.kill();done();});
      });
      else try {process.kill(-ownedPid,'SIGKILL');cleanup={status:'terminated_owned_tree',ownedPid};}catch(error){cleanup={status:'unknown',ownedPid,error:error.message};child.kill();}
    }, timeoutMs);
    child.on('error', error => { processError = error.message; });
    child.stdout.on('data', buffer => {
      boundedAppend(out, buffer, stdoutState, maxLogBytes);
      if (!onLine) return;
      for (const fragment of decoder.write(buffer).split(/(?<=\n)/u)) {
        if (!dropping) pending += fragment;
        if (Buffer.byteLength(pending) > 1024 * 1024) { pending = ''; dropping = true; parseTruncated = true; }
        if (fragment.endsWith('\n')) { if (!dropping && pending.trim()) onLine(pending.trim()); pending = ''; dropping = false; }
      }
    });
    child.stderr.on('data', buffer => boundedAppend(err, buffer, stderrState, maxLogBytes));
    child.on('close', (exitCode, signal) => {
      clearTimeout(timer);
      pending += decoder.end();
      if (onLine && pending.trim() && !dropping) onLine(pending.trim());
      resolve({ exitCode, signal });
    });
    child.stdin.on('error', () => {});
    child.stdin.end(input ?? '');
  });
  await cleanupPromise;
  fs.closeSync(out); fs.closeSync(err);
  return { ...result, elapsedMs: Math.round(performance.now() - start), timedOut, cleanup, processError, stdout: stdoutState, stderr: stderrState, parseTruncated };
}

function checkAssertions(value, assertions) {
  return assertions.map(assertion => {
    const actual = assertion.path.split('.').reduce((current, key) => current?.[key], value);
    const passed = 'equals' in assertion ? isDeepStrictEqual(actual, assertion.equals) : assertion.exists === true && actual !== undefined && actual !== null;
    return { path: assertion.path, passed, actual: actual ?? null, expected: assertion.equals ?? { exists: true } };
  });
}

export async function verifyCase(config, item, directory, metrics) {
  const results = [];
  for (const [index, check] of item.verifiers.entries()) {
    const prefix = path.join(directory, `verify-${index}`);
    const execution = await execute(check.command, check.args, { cwd: item.workspace, env:config.runtime?buildChildEnvironment(config):undefined, timeoutMs: config.timeoutMs, maxLogBytes: config.maxLogBytes, stdoutPath: `${prefix}.stdout`, stderrPath: `${prefix}.stderr` });
    let assertions = [], error = null, manifest = null;
    if (check.assertions?.length) {
      try {
        if (execution.stdout.truncated) throw new Error('Verifier JSON stdout truncated');
        const file = check.jsonFile ? path.resolve(item.workspace, check.jsonFile) : `${prefix}.stdout`;
        if (check.jsonFile) assertContained(item.workspace, file);
        const value = JSON.parse(fs.readFileSync(file, 'utf8'));
        assertions = checkAssertions(value, check.assertions);
        if (check.role === 'manifest') {
          const contract = value.facade ?? value;
          const operation = value.operations?.find(operation => operation.id === check.agentOperation);
          manifest = { version: contract.version ?? null, protocol: contract.protocol ?? null, operation: check.agentOperation, agentExposed: operation?.agent === true };
          manifest.passed = manifest.version === 4 && manifest.protocol === 3 && manifest.agentExposed;
        }
      } catch (failure) { error = failure.message; }
    }
    results.push({ role: check.role, command: check.command, args: check.args, ...execution, assertions, manifest, error, passed: execution.exitCode === 0 && !execution.timedOut && !execution.processError && !error && assertions.every(assertion => assertion.passed) && (check.role !== 'manifest' || manifest?.passed === true) });
  }
  if (item.id === 'packaged-mcp') {
    const calls = metrics.mcpResults.filter(call => call.server === 'revenant' && call.tool === item.expectedMcpTool && isDeepStrictEqual(call.arguments, item.expectedMcpArguments));
    results.push({ role: 'actual-mcp-call', passed: calls.some(call => call.status === 'completed' && !call.error && call.result != null && call.result.isError !== true), matchedCalls: calls.length });
  }
  for (const check of item.eventAssertions ?? []) {
    const calls = metrics.mcpResults.filter(call => call.server === 'revenant' && call.tool === check.tool && (!check.argumentsContain || Object.entries(check.argumentsContain).every(([key, value]) => isDeepStrictEqual(call.arguments?.[key], value))));
    const matches = calls.map(call => {
      let value = call.result?.structured_content ?? call.result?.structuredContent;
      if (value == null) {
        try { value = JSON.parse(call.result?.content?.find(content => content.type === 'text')?.text); } catch { value = null; }
      }
      const assertions = checkAssertions(value, check.assertions ?? []);
      return { assertions, passed: call.status === 'completed' && !call.error && call.result != null && assertions.every(assertion => assertion.passed) };
    });
    results.push({ role: `mcp-event:${check.tool}`, matchedCalls: calls.length, matches, passed: matches.some(match => match.passed) });
  }
  return results;
}

export async function prepareCase(config, item, directory) {
  const results = [];
  for (const [index, step] of (item.preparation ?? []).entries()) {
    const execution = await execute(step.command, step.args, { cwd: item.workspace, env:config.runtime?buildChildEnvironment(config):undefined, timeoutMs: config.timeoutMs, maxLogBytes: config.maxLogBytes, stdoutPath: path.join(directory, `setup-${index}.stdout`), stderrPath: path.join(directory, `setup-${index}.stderr`) });
    const result = { role: 'fixture-setup', command: step.command, args: step.args, ...execution, passed: execution.exitCode === 0 && !execution.timedOut && !execution.processError };
    results.push(result);
    if (!result.passed) break;
  }
  return results;
}

export function agentArguments(config, item) {
  validatePermissions(config);
  const override = value => JSON.stringify(value);
  // --sandbox would override the named permissions profile. Declare Windows
  // implementation separately because --ignore-user-config removes its default.
  const args = [...(config.codex.prefix ?? []), 'exec', '--json', '--ephemeral', '--ignore-user-config', '--disable','multi_agent','--disable','multi_agent_v2','--skip-git-repo-check', '-C', item.workspace, '-m', config.model, '-c', `model_reasoning_effort=${override(config.reasoningEffort)}`, '-c', `default_permissions=${override(config.permissionsProfile)}`];
  if (config.windowsSandbox) args.push('-c', `windows.sandbox=${override(config.windowsSandbox)}`);
  if (item.mcp) args.push('-c', `mcp_servers.revenant.command=${override(item.mcp.command)}`, '-c', `mcp_servers.revenant.args=${override(item.mcp.args)}`);
  return [...args, '-'];
}

export function verifyProvenance(config) {
  if(!config.provenance)return null;
  const digest=file=>createHash('sha256').update(fs.readFileSync(file)).digest('hex');
  if(digest(config.provenance.file)!==config.provenance.sha256)throw new Error('Frozen provenance record changed');
  const provenance=JSON.parse(fs.readFileSync(config.provenance.file,'utf8'));
  for(const item of provenance.files)if(digest(item.path)!==item.sha256)throw new Error(`Frozen artifact changed: ${item.path}`);
  return provenance;
}

export async function runEvaluation(config, { prepareOnly = false } = {}) {
  validateConfiguration(config);
  const runtime=verifyRuntime(config),preflight=verifyPreflight(config),env=buildChildEnvironment(config);
  const provenance=verifyProvenance(config);
  if (fs.existsSync(config.output)) throw new Error('Output directory already exists; preserve earlier evaluation evidence');
  fs.mkdirSync(config.output, { recursive: true });
  if(provenance)fs.writeFileSync(path.join(config.output,'provenance.json'),JSON.stringify(provenance,null,2)+'\n',{flag:'wx'});
  fs.writeFileSync(path.join(config.output,'runtime.json'),JSON.stringify({runtime,environmentPolicy:environmentPolicy(config),preflight},null,2)+'\n',{flag:'wx'});
  const version = await execute(config.codex.command, [...(config.codex.prefix ?? []), '--version'], { cwd: config.allowedRoot, env, timeoutMs: 10000, maxLogBytes: 4096, stdoutPath: path.join(config.output, 'codex-version.stdout'), stderrPath: path.join(config.output, 'codex-version.stderr') });
  if (version.exitCode !== 0) throw new Error('Unable to record actual Codex CLI version');
  const cliVersion = fs.readFileSync(path.join(config.output, 'codex-version.stdout'), 'utf8').trim();
  if(cliVersion!=='codex-cli 0.159.1')throw new Error(`Unexpected pinned CLI version: ${cliVersion}`);
  const frozen = { schemaVersion: 1, createdAt: new Date().toISOString(), nodeVersion: process.version, cliVersion, config, cases: fixedCases.map(spec => {
    const item = config.cases.find(item => item.id === spec.id);
    return { ...spec, workspace: item.workspace, prompt: `${spec.prompt}\n\nFixed case context:\n${item.context ?? ''}\n\nAll project edits must stay within ${item.workspace}. Do not publish, commit, push, modify credentials, or operate other projects. Complete this in one uninterrupted turn; no human interventions are provided.`, args: agentArguments(config, item) };
  }) };
  const serialized = JSON.stringify(frozen, null, 2);
  fs.writeFileSync(path.join(config.output, 'configuration.json'), `${serialized}\n`, { flag: 'wx' });
  const configurationSha256 = createHash('sha256').update(serialized).digest('hex');
  if (prepareOnly) return { prepared: true, cliVersion, configurationSha256, output: config.output };
  const results = [];
  for (const spec of frozen.cases) {
    const item = config.cases.find(item => item.id === spec.id), metrics = createEventMetrics();
    const directory = path.join(config.output, spec.id);
    fs.mkdirSync(directory);
    const preparation = await prepareCase(config, item, directory);
    const setupPassed = preparation.every(step => step.passed);
    if (!setupPassed) metrics.accept({ type: 'error', message: 'Predeclared fixture preparation failed; model was not run' });
    const execution = setupPassed ? await execute(config.codex.command, spec.args, { cwd: item.workspace, env, timeoutMs: config.timeoutMs, maxLogBytes: config.maxLogBytes, stdoutPath: path.join(directory, 'events.jsonl'), stderrPath: path.join(directory, 'stderr.log'), input: spec.prompt, onLine(line) { try { metrics.accept(JSON.parse(line)); } catch { metrics.malformed(); } } }) : { exitCode: null, elapsedMs: 0, timedOut: false, stdout: null, stderr: null, parseTruncated: false };
    if (execution.processError) metrics.accept({ type: 'error', message: execution.processError });
    if (execution.parseTruncated) metrics.malformed();
    const snapshot = metrics.snapshot();
    // Only actual exec failure events qualify. Do not continue spending model
    // calls or injecting faults when the account cannot run another turn.
    const usageLimited = snapshot.failures.some(message => /\byou['’]ve hit your usage limit\b|\busage_limit_reached\b/i.test(message));
    const verifications = setupPassed && !execution.timedOut && !usageLimited ? await verifyCase(config, item, directory, snapshot) : [];
    const result = { id: spec.id, preparation, cleanup:execution.cleanup??null, ...summarizeRun({ metrics: snapshot, ...execution, verifications, interventionCount: 0 }), logs: { stdout: execution.stdout, stderr: execution.stderr, parseTruncated: execution.parseTruncated } };
    results.push(result);
    fs.writeFileSync(path.join(directory, 'result.json'), `${JSON.stringify(result, null, 2)}\n`);
    fs.writeFileSync(path.join(config.output, 'results.json'), `${JSON.stringify({ schemaVersion: 1, configurationSha256, cliVersion, model: config.model, reasoningEffort: config.reasoningEffort, results }, null, 2)}\n`);
    process.stdout.write(`${spec.id}: agent=${result.agentCompleted}, independently verified=${result.completed}\n`);
    if (usageLimited) {
      fs.writeFileSync(path.join(config.output, 'aborted.json'), JSON.stringify({ reason: 'Account usage limit; evaluation stops before subsequent cases', case: spec.id, unattemptedCases: frozen.cases.slice(results.length).map(item => item.id) }, null, 2) + '\n');
      break;
    }
    if(execution.timedOut||preparation.some(step=>step.timedOut)||verifications.some(step=>step.timedOut)) {
      fs.writeFileSync(path.join(config.output,'aborted.json'),JSON.stringify({reason:'Timed-out owned process; evaluation stops before subsequent cases',case:spec.id,cleanup:result.cleanup},null,2)+'\n');
      break;
    }
  }
  return { output: config.output, configurationSha256, results };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.includes('--help')) {
    process.stdout.write('Fixed permissions: permissionsProfile=":workspace"; on Windows also windowsSandbox="elevated" (prepared native sandbox). Managed rules remain active.\n');
    process.stdout.write('Runtime prerequisite: runtime={schemaVersion:1,root,package:{name:"@openai/codex",version:"0.159.1",integrity,packageJson,lockFile,platform?:{name:"@openai/codex-win32-x64",version:"0.159.1-win32-x64",integrity,packageJson}},files:[{role,path,sha256}]}; required roles cli,codeModeHost,commandRunner,sandboxSetup,node. Platform package required on Windows. Every path must be inside the private runtime root beneath allowedRoot.\n');
    process.stdout.write('Preflight prerequisite: preflight={file,sha256} binds a successful schemaVersion=1 report with workspace,runtimeSha256,environmentPolicySha256,permissions matching the fixed profile,checks={command,patchCreate,patchUpdate,bytesVerified,dependencies} all true, final patch proof={file,sha256,expectedText}, and frozen records={events,execution,dependencies}, each {file,sha256}. Actual events must contain successful command and distinct completed add/update patches. Dependency records require npm-version,cargo-version,npm-cache,cargo-cache executions; cache args include --offline. Use exported environmentPolicy/buildChildEnvironment for the same isolated launch environment. Prerequisites are checked before any CLI process or fixture setup.\n');
    process.stdout.write('Usage: node scripts/evaluate-agent-workflow.mjs --config <json> [--prepare]\nConfig: model=gpt-6.1-sol, reasoningEffort=xhigh, codex={command,prefix?}, allowedRoot, output, timeoutMs, maxLogBytes, cases=[{id,workspace,context,mcp?:{command,args},verifiers:[{role,command,args,jsonFile?,agentOperation?,assertions?:[{path,equals|exists}]}],eventAssertions?:[{tool,argumentsContain?,assertions:[{path,equals|exists}]}],expectedMcpTool?,expectedMcpArguments?}].\nFive ids: scaffold, luma-feature, contract-repair, native-failure, packaged-mcp. Roots must already be prepared and isolated. Manifest verifiers require agentOperation and check version=4/protocol=3/exposure=true. Native-failure requires structured MCP event assertions. MCP is optional only for scaffold. Verifiers run independently after the agent and must not trust its claimed success. --prepare records fixed configuration without running agents. Use a new output directory per run.\n');
  } else {
    const index = args.indexOf('--config');
    if (index < 0 || !args[index + 1]) { process.stderr.write('Missing --config <json>; use --help\n'); process.exitCode = 2; }
    else try {
      const result = await runEvaluation(JSON.parse(fs.readFileSync(args[index + 1], 'utf8')), { prepareOnly: args.includes('--prepare') });
      if (result.prepared) process.stdout.write(`${JSON.stringify(result)}\n`);
      else if (result.results.some(item => !item.completed)) process.exitCode = 1;
    } catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
  }
}
