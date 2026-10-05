/** Real rust-analyzer stdio evidence; no source, preference, or dependency edits.
 * Usage: node scripts/check-ide.mjs --project <app> --allow-cargo [--build-scripts] [--zed]
 * rust-analyzer uses Cargo metadata/build scripts: coordinate the Cargo slot first.
 * JSON goes to stdout; keep it in the caller's evidence store, not this script.
 */
import fs from 'node:fs';
import path from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(name);
  return index < 0 ? fallback : args[index + 1];
}
const project = path.resolve(option('--project', '.'));
const timeout = Number(option('--timeout-ms', '180000'));
const allowCargo = args.includes('--allow-cargo');
const evidence = { project, status: 'pending', visualHoverVerified: false, cargoCoordinated: allowCargo };
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

function read(file) { return fs.readFileSync(file, 'utf8'); }
function position(text, offset) {
  const prefix = text.slice(0, offset).split('\n');
  return { line: prefix.length - 1, character: prefix.at(-1).length };
}
function serverBinary() {
  const candidate = option('--server', 'rust-analyzer');
  try {
    evidence.serverVersion = execFileSync(candidate, ['--version'], { encoding: 'utf8', timeout: 10000 }).trim();
    return candidate;
  } catch {
    // Zed manages real server binaries independently of the rustup shim.
    const roots = [process.env.LOCALAPPDATA, process.env.APPDATA]
      .filter(Boolean).map(root => path.join(root, 'Zed', 'languages', 'rust-analyzer'));
    const find = (directory, depth = 0) => {
      if (depth > 4 || !fs.existsSync(directory)) return [];
      return fs.readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
        const file = path.join(directory, entry.name);
        return entry.isDirectory() ? find(file, depth + 1)
          : /^rust-analyzer(?:\.exe)?$/.test(entry.name) ? [file] : [];
      });
    };
    for (const file of roots.flatMap(root => find(root))) {
      try {
        evidence.serverVersion = execFileSync(file, ['--version'], { encoding: 'utf8', timeout: 10000 }).trim();
        return file;
      } catch { /* A stale cached server is not usable evidence. */ }
    }
    throw new Error('No working rust-analyzer; pass --server with an installed binary.');
  }
}

class Lsp {
  constructor(binary) {
    this.next = 1;
    this.pending = new Map();
    this.buffer = Buffer.alloc(0);
    this.stderr = '';
    this.status = null;
    this.child = spawn(binary, [], { cwd: project, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    this.child.stdout.on('data', chunk => { this.buffer = Buffer.concat([this.buffer, chunk]); this.consume(); });
    this.child.stderr.on('data', chunk => { this.stderr = (this.stderr + chunk.toString()).slice(-4000); });
    this.child.on('error', error => this.fail(error));
    this.child.on('exit', code => this.fail(new Error(`rust-analyzer exited (${code})`)));
    this.child.stdin.on('error', error => this.fail(error));
  }
  fail(error) {
    for (const waiter of this.pending.values()) { clearTimeout(waiter.timer); waiter.reject(error); }
    this.pending.clear();
  }
  send(message) {
    const bytes = Buffer.from(JSON.stringify({ jsonrpc: '2.0', ...message }));
    this.child.stdin.write(`Content-Length: ${bytes.length}\r\n\r\n`);
    this.child.stdin.write(bytes);
  }
  notify(method, params) { this.send({ method, params }); }
  request(method, params) {
    const id = this.next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Timed out: ${method}`)); }, timeout);
      this.pending.set(id, { resolve, reject, timer });
      this.send({ id, method, params });
    });
  }
  consume() {
    while (true) {
      const end = this.buffer.indexOf('\r\n\r\n');
      if (end < 0) return;
      const match = /Content-Length:\s*(\d+)/i.exec(this.buffer.subarray(0, end).toString());
      if (!match) return this.fail(new Error('Invalid LSP header'));
      const length = Number(match[1]);
      if (this.buffer.length < end + 4 + length) return;
      const message = JSON.parse(this.buffer.subarray(end + 4, end + 4 + length).toString());
      this.buffer = this.buffer.subarray(end + 4 + length);
      if (message.method === 'experimental/serverStatus') this.status = message.params;
      if (message.method && message.id !== undefined) {
        const result = message.method === 'workspace/configuration'
          ? message.params.items.map(() => this.config) : null;
        this.send({ id: message.id, result });
      } else if (message.id !== undefined) {
        const waiter = this.pending.get(message.id);
        if (waiter) {
          clearTimeout(waiter.timer);
          this.pending.delete(message.id);
          if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
          else waiter.resolve(message.result);
        }
      }
    }
  }
  async close() {
    try { await Promise.race([this.request('shutdown', null), sleep(3000)]); } catch { /* Already exited. */ }
    if (this.child.exitCode === null) this.notify('exit');
    await Promise.race([new Promise(resolve => this.child.once('exit', resolve)), sleep(1500)]);
    if (this.child.exitCode === null) this.child.kill();
    this.fail(new Error('LSP session closed'));
  }
}

function hoverText(hover) {
  const value = hover?.contents;
  return (Array.isArray(value) ? value : [value]).filter(Boolean)
    .map(item => typeof item === 'string' ? item : item.value ?? '').join('\n');
}
function definitions(result) {
  return (Array.isArray(result) ? result : result ? [result] : []).map(location => {
    const uri = location.targetUri ?? location.uri;
    return { file: uri?.startsWith('file:') ? fileURLToPath(uri) : uri,
      range: location.targetSelectionRange ?? location.range };
  });
}

function typescriptEvidence() {
  const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const require = createRequire(import.meta.url);
  const ts = require(path.join(repo, 'packages', 'client', 'node_modules', 'typescript'));
  const forwarder = path.join(project, 'web', 'src', 'lib', 'revenant.ts');
  const reference = /export \* from ['"]([^'"]+)['"]/.exec(read(forwarder));
  if (!reference) throw new Error('Generated TypeScript forwarder has no facade export.');
  const file = path.resolve(path.dirname(forwarder), reference[1] + '.ts');
  const source = read(file);
  const needle = /["']normalize["']\s*:\s*Operation</.exec(source);
  if (!needle) throw new Error('Current facade has no typed records.normalize declaration.');
  const usageFile = path.join(path.dirname(file), '__ide_hover_probe__.ts');
  const usage = "import type { Application } from './facade';\ndeclare const app: Application;\napp.operations.records.normalize;\n";
  const offset = usage.indexOf('normalize') + 2;
  const isUsage = filename => path.resolve(filename).toLowerCase() === usageFile.toLowerCase();
  const options = { target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler, skipLibCheck: true };
  const host = {
    getScriptFileNames: () => [file, usageFile], getScriptVersion: () => '1',
    getScriptSnapshot: filename => isUsage(filename) ? ts.ScriptSnapshot.fromString(usage)
      : fs.existsSync(filename) ? ts.ScriptSnapshot.fromString(read(filename)) : undefined,
    getCurrentDirectory: () => project, getCompilationSettings: () => options,
    getDefaultLibFileName: settings => ts.getDefaultLibFilePath(settings),
    fileExists: filename => isUsage(filename) || ts.sys.fileExists(filename),
    readFile: filename => isUsage(filename) ? usage : ts.sys.readFile(filename), readDirectory: ts.sys.readDirectory,
  };
  const service = ts.createLanguageService(host);
  try {
    const quick = service.getQuickInfoAtPosition(usageFile, offset);
    const docs = ts.displayPartsToString(quick?.documentation);
    const signature = ts.displayPartsToString(quick?.displayParts);
    const targets = service.getDefinitionAtPosition(usageFile, offset) ?? [];
    const links = [...docs.matchAll(/\[Rust source\]\(([^)]+)#L(\d+)\)/g)].map(match => {
      const target = path.resolve(path.dirname(file), decodeURIComponent(match[1]));
      const line = Number(match[2]);
      return { file: target, line, exists: fs.existsSync(target),
        sourceLine: fs.existsSync(target) ? read(target).split(/\r?\n/)[line - 1]?.trim().slice(0, 300) : null };
    });
    return { version: ts.version, file, symbol: 'records.normalize',
      hover: { signature, documentation: docs.slice(0, 4500), tags: quick?.tags },
      generatedDefinitions: targets.map(target => ({ file: target.fileName,
        position: position(read(target.fileName), target.textSpan.start) })),
      rustSourceLinks: links,
      verified: docs.includes('Trim and uppercase') && links.some(link => link.exists && link.sourceLine?.includes('operation'))
        && targets.length > 0,
      visualHoverVerified: false };
  } finally { service.dispose(); }
}

async function zedEvidence(file) {
  const logs = [process.env.LOCALAPPDATA, process.env.APPDATA].filter(Boolean)
    .map(root => path.join(root, 'Zed', 'logs', 'Zed.log'));
  const log = logs.find(file => fs.existsSync(file));
  const offset = log ? fs.statSync(log).size : 0;
  const cli = spawn(option('--zed-bin', 'zed'), ['--new', project, file],
    { stdio: 'ignore', windowsHide: true });
  await new Promise((resolve, reject) => { cli.once('error', reject); cli.once('exit', resolve); });
  await sleep(8000);
  let lines = [];
  if (log && fs.existsSync(log)) {
    const size = fs.statSync(log).size;
    const start = size < offset ? Math.max(0, size - 128000) : Math.max(offset, size - 128000);
    const fd = fs.openSync(log, 'r');
    try {
      const buffer = Buffer.alloc(size - start);
      fs.readSync(fd, buffer, 0, buffer.length, start);
      lines = buffer.toString().split(/\r?\n/)
        .filter(line => /rust-analyzer|language server|worktree|workspace|cargo|project/i.test(line))
        .slice(-25).map(line => line.slice(0, 700));
    } finally { fs.closeSync(fd); }
  }
  let processes = [];
  if (process.platform === 'win32') {
    const command = "$p=$env:CHECK_IDE_PROJECT; $all=@(Get-CimInstance Win32_Process); $byId=@{}; foreach($item in $all){$byId[[int]$item.ProcessId]=$item}; $items=@($all | Where-Object { $_.Name -like '*zed*.exe' -or ($_.Name -eq 'cargo.exe' -and $_.CommandLine -like ('*'+$p+'*')) } | ForEach-Object { $ancestor=$_.ParentProcessId; $fromZed=$false; for($i=0;$i -lt 12;$i++){ $parent=$byId[[int]$ancestor]; if(!$parent){break}; if($parent.Name -like '*zed*.exe'){$fromZed=$true;break}; $ancestor=$parent.ParentProcessId }; [PSCustomObject]@{ pid=$_.ProcessId; parent=$_.ParentProcessId; name=$_.Name; executable=$_.ExecutablePath; zedDescendant=$fromZed; projectCommand=$(if ($_.Name -eq 'cargo.exe') { $_.CommandLine } else { $null }) } }); ConvertTo-Json -InputObject $items -Compress";
    try {
      processes = JSON.parse(execFileSync('powershell.exe', ['-NoProfile', '-Command', command], {
        encoding: 'utf8', timeout: 10000, windowsHide: true,
        env: { ...process.env, CHECK_IDE_PROJECT: project },
      }));
    } catch { /* Process access may be restricted; fresh logs remain evidence. */ }
  }
  const normalized = value => value.replaceAll('\\\\', '\\').replaceAll('\\', '/').toLowerCase();
  const discovered = lines.some(line => normalized(line).includes(normalized(project)))
    || processes.some(process => process.zedDescendant && process.projectCommand?.includes(project));
  return { requestedProject: project, requestedFile: file, cliExit: cli.exitCode,
    log, freshLogLines: lines, processes, discoveryVerified: discovered, visualHoverVerified: false };
}

let lsp;
try {
  if (args.includes('--typescript-only')) {
    evidence.typescript = typescriptEvidence();
    evidence.status = evidence.typescript.verified ? 'passed' : 'failed';
  } else {
  if (!allowCargo) throw new Error('Coordinate Cargo first, then pass --allow-cargo (rust-analyzer loads Cargo workspaces).');
  const native = path.join(project, 'native', 'Cargo.toml');
  const bootstrap = path.join(project, '.revenant', 'desktop', 'Cargo.toml');
  const linkedProjects = [native, bootstrap].filter(file => fs.existsSync(file));
  if (!linkedProjects.length) throw new Error('No native or hidden bootstrap Cargo manifest found.');
  const settingsFile = path.join(project, '.zed', 'settings.json');
  evidence.zedProjectSettings = fs.existsSync(settingsFile)
    ? { file: settingsFile, linkedProjects: JSON.parse(read(settingsFile)).lsp?.['rust-analyzer']?.initialization_options?.linkedProjects ?? [] }
    : null;
  evidence.linkedProjects = linkedProjects;
  const file = path.resolve(option('--file', fs.existsSync(native)
    ? path.join(project, 'native', 'src', 'lib.rs') : path.join(project, '.revenant', 'desktop', 'src', 'main.rs')));
  const text = read(file);
  evidence.openedFile = file;
  const probes = [
    { name: 'Application::new', match: /Application::(new)/, expected: /Creates a definition with the standard desktop capabilities/ },
    { name: 'operations!', match: /(operations)!\[/, expected: /Composes.*operations/s },
  ].flatMap(probe => {
    const match = probe.match.exec(text);
    return match ? [{ ...probe, position: position(text, match.index + match[0].indexOf(match[1])) }] : [];
  });
  if (!probes.some(probe => probe.name === 'Application::new')) throw new Error('No Application::new() call found in the opened file.');
  const binary = serverBinary();
  evidence.server = binary;
  lsp = new Lsp(binary);
  lsp.config = { linkedProjects, checkOnSave: false, check: { enable: false },
    cargo: { buildScripts: { enable: args.includes('--build-scripts') }, targetDir: path.join(project, '.revenant', 'target') },
    procMacro: { enable: true } };
  await lsp.request('initialize', {
    processId: process.pid, rootUri: pathToFileURL(project).href,
    workspaceFolders: [{ uri: pathToFileURL(project).href, name: path.basename(project) }],
    initializationOptions: lsp.config,
    capabilities: { workspace: { configuration: true, workspaceFolders: true },
      textDocument: { hover: { contentFormat: ['markdown', 'plaintext'] }, definition: { linkSupport: true } },
      experimental: { serverStatusNotification: true } },
  });
  lsp.notify('initialized', {});
  const uri = pathToFileURL(file).href;
  lsp.notify('textDocument/didOpen', { textDocument: { uri, languageId: 'rust', version: 1, text } });
  const deadline = Date.now() + timeout;
  evidence.probes = [];
  for (const probe of probes) {
    let hover = null, targets = [], lastError;
    do {
      try {
        hover = await lsp.request('textDocument/hover', { textDocument: { uri }, position: probe.position });
        targets = definitions(await lsp.request('textDocument/definition', { textDocument: { uri }, position: probe.position }));
      } catch (error) { lastError = error.message; }
      if (hoverText(hover) && targets.length) break;
      await sleep(1000);
    } while (Date.now() < deadline);
    const docs = hoverText(hover);
    const expectedSource = probe.name === 'Application::new' ? 'application.rs' : 'lib.rs';
    const verified = probe.expected.test(docs) && targets.some(target => target.file?.endsWith(expectedSource));
    evidence.probes.push({ name: probe.name, position: probe.position, hover: docs.slice(0, 4500),
      definitions: targets, verified, ...(lastError && !verified ? { error: lastError } : {}) });
    if (probe.name === 'operations!' && args.includes('--build-scripts')) {
      let expanded = null;
      do {
        try {
          expanded = await lsp.request('rust-analyzer/expandMacro', { textDocument: { uri }, position: probe.position });
        } catch (error) { lastError = error.message; }
        if (expanded?.expansion?.includes('normalize_operation')) break;
        await sleep(1000);
      } while (Date.now() < deadline);
      evidence.procMacro = { name: expanded?.name, expansion: expanded?.expansion?.slice(0, 4500),
        verified: Boolean(expanded?.expansion?.includes('normalize_operation')),
        ...(!expanded && lastError ? { error: lastError } : {}) };
    }
  }
  evidence.serverStatus = lsp.status;
  if (args.includes('--zed')) evidence.zed = await zedEvidence(file);
  if (args.includes('--typescript')) evidence.typescript = typescriptEvidence();
  evidence.status = evidence.probes.every(probe => probe.verified)
    && (!args.includes('--zed') || evidence.zed.discoveryVerified)
    && (!evidence.procMacro || evidence.procMacro.verified)
    && (!args.includes('--typescript') || evidence.typescript.verified) ? 'passed' : 'failed';
  }
} catch (error) {
  evidence.status = 'failed';
  evidence.error = error.message;
} finally {
  if (lsp) await lsp.close();
  console.log(JSON.stringify(evidence, null, 2));
  process.exitCode = evidence.status === 'passed' ? 0 : 1;
}
