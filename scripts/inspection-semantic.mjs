/** Development inspection through real language services. JSON stdin/stdout only.
 * Source roots are bounded; generated artifacts are opened only by exact path.
 * Compiler/LSP results retain their provenance and incomplete coverage is explicit.
 */
import fs from 'node:fs';
import path from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { pathToFileURL, fileURLToPath } from 'node:url';

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const excluded = new Set(['node_modules','.git','.revenant','target','build','dist','.svelte-kit']);
function canonical(file) { return fs.realpathSync(file); }
function inside(root, file) {
  const relative = path.relative(root, file);
  return !relative.startsWith('..' + path.sep) && relative !== '..' && !path.isAbsolute(relative);
}
function allowed(root, file) {
  const resolved = canonical(path.resolve(root, file));
  if (!inside(root, resolved)) throw new Error('Source path escapes project root');
  if (fs.statSync(resolved).size > 4 * 1024 * 1024) throw new Error('Source file exceeds inspection budget');
  return resolved;
}
function read(file) { return fs.readFileSync(file, 'utf8'); }
function location(root, file, text, start, length = 0, provenance) {
  const prefix = text.slice(0, start).split('\n');
  const end = text.slice(0, start + length).split('\n');
  return {file:path.relative(root,file).split(path.sep).join('/'),line:prefix.length,column:prefix.at(-1).length+1,
    endLine:end.length,endColumn:end.at(-1).length+1,provenance};
}
function sourceFiles(root) {
  const result = {files:[],truncated:false,visitedEntries:0};
  let bytes = 0;
  function visit(directory, depth = 0) {
    if (depth > 32 || result.files.length >= 4096 || bytes >= 32*1024*1024 || result.visitedEntries>=8192) { result.truncated = true; return; }
    if (!fs.existsSync(directory)) return;
    const reader=fs.opendirSync(directory);
    try {
      while(true) {
        const entry=reader.readSync();if(!entry)break;
        if(result.visitedEntries>=8192||result.files.length>=4096||bytes>=32*1024*1024){result.truncated=true;break;}
        result.visitedEntries++;
        if (entry.isSymbolicLink()) continue;
        const file = path.join(directory, entry.name);
        if (entry.isDirectory()) { if (!excluded.has(entry.name)) visit(file, depth+1); }
        else if (/\.(?:ts|tsx|js|jsx|svelte|rs)$/.test(entry.name)) {
          const size=fs.statSync(file).size;
          if(size>4*1024*1024 || bytes+size>32*1024*1024) {result.truncated=true;continue;}
          bytes+=size; result.files.push(file);
        }
      }
    } finally {reader.closeSync();}
  }
  visit(path.join(root,'web/src')); visit(path.join(root,'native/src'));
  result.files.sort();
  return result;
}
function dependency(request, name) {
  const roots = [request.dependencyRoot, path.join(request.project,'web'), path.join(request.project,'.revenant/sdk/packages/client')].filter(Boolean);
  for(const root of roots) {
    const require = createRequire(path.join(path.resolve(root),'package.json'));
    try {return {require,file:require.resolve(name)};} catch { /* Try the next installed dependency root. */ }
  }
  throw new Error(`${name} is not installed; install the application's declared frontend development dependencies`);
}
function typeService(request, files) {
  const {require,file} = dependency(request,'typescript');
  const ts = require(file);
  const web=path.join(request.project,'web');
  const configFile=ts.findConfigFile(web,ts.sys.fileExists,'tsconfig.json');
  const config=configFile ? ts.readConfigFile(configFile,ts.sys.readFile) : {config:{}};
  const parsed=ts.parseJsonConfigFileContent(config.config ?? {},ts.sys,web,{
    allowJs:true,skipLibCheck:true,target:ts.ScriptTarget.ESNext,module:ts.ModuleKind.ESNext,moduleResolution:ts.ModuleResolutionKind.Bundler,
  },configFile);
  const names=files.filter(file=>/\.(?:ts|tsx|js|jsx)$/.test(file));
  if (request.facade) names.push(allowed(request.project,request.facade));
  const host={getScriptFileNames:()=>[...new Set(names)],getScriptVersion:()=> '1',
    getScriptSnapshot:filename=>fs.existsSync(filename) && fs.statSync(filename).size<=4*1024*1024 ? ts.ScriptSnapshot.fromString(read(filename)) : undefined,
    getCurrentDirectory:()=>web,getCompilationSettings:()=>parsed.options,
    getDefaultLibFileName:settings=>ts.getDefaultLibFilePath(settings),
    fileExists:ts.sys.fileExists,readFile:ts.sys.readFile,readDirectory:ts.sys.readDirectory,
    directoryExists:ts.sys.directoryExists,getDirectories:ts.sys.getDirectories,realpath:ts.sys.realpath,
  };
  return {ts,service:ts.createLanguageService(host),configDiagnostics:[...(config.error?[config.error]:[]),...parsed.errors]};
}
function bindingPositions(ts, file, id) {
  const text=read(file), source=ts.createSourceFile(file,text,ts.ScriptTarget.Latest,true);
  let candidates=[];
  function walk(node, properties=[], owner='') {
    if(ts.isInterfaceDeclaration(node) || ts.isTypeAliasDeclaration(node)) { owner=node.name.text; properties=[]; }
    let next=properties;
    if(ts.isPropertySignature(node) || ts.isPropertyAssignment(node)) {
      const name=node.name && (ts.isIdentifier(node.name)||ts.isStringLiteral(node.name)) ? node.name.text : null;
      if(name) {
        next=[...properties,name];
        const symbolPath=next.join('.');
        const callable=owner==='Operations' && symbolPath===id;
        const application=owner==='Application' && (symbolPath===`operations.${id}` || (/^(?:files|media)\./.test(id)&&symbolPath===id));
        if(callable || application)candidates.push({node,owner,symbolPath,score:callable?0:1});
      }
    }
    ts.forEachChild(node,child=>walk(child,next,owner));
  }
  walk(source);
  candidates.sort((a,b)=>a.score-b.score);
  return candidates.map(({node,owner,symbolPath})=>({offset:node.name.getStart(source)+(ts.isStringLiteral(node.name)?1:0),length:node.name.text.length,text,owner,symbolPath}));
}

class Lsp {
  constructor(program, args, cwd, timeout) {
    this.next=1;this.pending=new Map();this.buffer=Buffer.alloc(0);this.timeout=timeout;this.diagnostics=new Map();this.status=null;
    this.child=spawn(program,args,{cwd,stdio:['pipe','pipe','pipe'],windowsHide:true});
    this.closed=new Promise(resolve=>this.child.once('close',resolve));
    this.child.stdout.on('data',data=>{this.buffer=Buffer.concat([this.buffer,data]);if(this.buffer.length>8*1024*1024){this.fail(new Error('Language server output exceeds budget'));this.child.kill();return;}this.consume();});
    this.child.stderr.on('data',()=>{});
    this.child.on('error',error=>this.fail(error));this.child.on('exit',code=>this.fail(new Error(`Language server exited (${code})`)));
    this.child.stdin.on('error',error=>this.fail(error));
  }
  fail(error) {this.error=error;for(const waiter of this.pending.values()){clearTimeout(waiter.timer);waiter.reject(error);}this.pending.clear();}
  send(message) {if(this.error)throw this.error;const bytes=Buffer.from(JSON.stringify({jsonrpc:'2.0',...message}));this.child.stdin.write(`Content-Length: ${bytes.length}\r\n\r\n`);this.child.stdin.write(bytes);}
  notify(method,params){this.send({method,params});}
  request(method,params){const id=this.next++;return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{this.pending.delete(id);reject(new Error(`Timed out: ${method}`));},this.timeout);this.pending.set(id,{resolve,reject,timer});try{this.send({id,method,params});}catch(error){clearTimeout(timer);this.pending.delete(id);reject(error);}});}
  consume(){while(true){const end=this.buffer.indexOf('\r\n\r\n');if(end<0)return;const match=/Content-Length:\s*(\d+)/i.exec(this.buffer.subarray(0,end).toString());if(!match){this.fail(new Error('Invalid language-server framing'));return;}const length=Number(match[1]);if(length>8*1024*1024){this.fail(new Error('Language-server message exceeds budget'));return;}if(this.buffer.length<end+4+length)return;let message;try{message=JSON.parse(this.buffer.subarray(end+4,end+4+length));}catch(error){this.fail(error);return;}this.buffer=this.buffer.subarray(end+4+length);
    if(message.method==='textDocument/publishDiagnostics')this.diagnostics.set(message.params.uri,message.params.diagnostics);
    if(message.method==='experimental/serverStatus')this.status=message.params;
    if(message.method && message.id!==undefined){const result=message.method==='workspace/configuration'?message.params.items.map(()=>this.config):null;this.send({id:message.id,result});}
    else if(message.id!==undefined){const waiter=this.pending.get(message.id);if(waiter){clearTimeout(waiter.timer);this.pending.delete(message.id);message.error?waiter.reject(new Error(message.error.message)):waiter.resolve(message.result);}}
  }}
  async close(){try{await Promise.race([this.request('shutdown',null),pause(3000)]);}catch{}try{if(this.child.exitCode===null)this.notify('exit');}catch{}await Promise.race([this.closed,pause(1500)]);if(this.child.exitCode===null){this.child.kill();await Promise.race([this.closed,pause(1500)]);}this.fail(new Error('Language-server session closed'));}
}
async function initialize(lsp,root,config) {
  lsp.config=config;
  await lsp.request('initialize',{processId:process.pid,rootUri:pathToFileURL(root).href,workspaceFolders:[{uri:pathToFileURL(root).href,name:path.basename(root)}],initializationOptions:config,
    capabilities:{workspace:{configuration:true,workspaceFolders:true},textDocument:{definition:{linkSupport:true},references:{},diagnostic:{dynamicRegistration:false},publishDiagnostics:{relatedInformation:true}},experimental:{serverStatusNotification:true}}});
  lsp.notify('initialized',{});
}
function lspLocations(root,result,provenance) {
  return (Array.isArray(result)?result:result?[result]:[]).flatMap(item=>{
    const uri=item.targetUri??item.uri,range=item.targetSelectionRange??item.range;
    if(!uri?.startsWith('file:')||!range)return [];
    const file=fileURLToPath(uri);
    if(!inside(root,file))return [];
    return [{file:path.relative(root,file).split(path.sep).join('/'),line:range.start.line+1,column:range.start.character+1,endLine:range.end.line+1,endColumn:range.end.character+1,provenance}];
  });
}
function offsetAt(text,line,column) {const lines=text.split('\n');if(line<1||line>lines.length||column<1||column>lines[line-1].length+1)throw new Error('Position is outside source file');return lines.slice(0,line-1).reduce((size,line)=>size+line.length+1,0)+column-1;}
function diagnostic(root,ts,entry,stage='typescript') {
  const span=entry.file?location(root,entry.file.fileName,entry.file.text,entry.start??0,entry.length??0,'typescript_language_service'):{};
  return {stage,code:String(entry.code),severity:entry.category===ts.DiagnosticCategory.Error?'error':entry.category===ts.DiagnosticCategory.Warning?'warning':'information',message:ts.flattenDiagnosticMessageText(entry.messageText,'\n').slice(0,4000),...span};
}
async function svelteEvidence(request,files,file,offset,action,bindings=[]) {
  const metadata=dependency(request,'svelte-language-server/package.json');
  const packageRoot=path.dirname(metadata.file),pkg=JSON.parse(read(metadata.file));
  const bin=typeof pkg.bin==='string'?pkg.bin:Object.values(pkg.bin??{})[0];
  if(!bin)throw new Error('svelte-language-server package has no binary');
  const lsp=new Lsp(process.execPath,[path.join(packageRoot,bin),'--stdio'],path.join(request.project,'web'),Math.min(request.timeoutMs,15000));
  try {
    await initialize(lsp,path.join(request.project,'web'),{typescript:{enable:true},svelte:{enable:true}});
    const components=files.filter(source=>source.endsWith('.svelte')).slice(0,128);
    for(const source of components)lsp.notify('textDocument/didOpen',{textDocument:{uri:pathToFileURL(source).href,languageId:'svelte',version:1,text:read(source)}});
    if(action==='diagnostics') {
      const reports=new Map(),deadline=Date.now()+Math.min(request.timeoutMs,20000);
      for(const component of components) {
        if(Date.now()>deadline)break;
        const uri=pathToFileURL(component).href;
        const report=await lsp.request('textDocument/diagnostic',{textDocument:{uri}});
        reports.set(uri,report?.items??[]);
      }
      return {indexedComponents:reports.size,complete:reports.size===files.filter(source=>source.endsWith('.svelte')).length,
        diagnostics:[...reports.entries()].flatMap(([uri,items])=>items.map(item=>({stage:'svelte',code:item.code===undefined?null:String(item.code),severity:item.severity===1?'error':item.severity===2?'warning':'information',message:item.message.slice(0,4000),...lspLocations(request.project,{uri,range:item.range},'svelte_language_server')[0]})))};
    }
    if(action==='operation') {
      // The Svelte server accepts Svelte documents. Candidate property positions
      // come from the compiler AST; definitions, rather than spelling, prove an
      // edge to this exact generated binding. References are server-mapped back
      // from Svelte's generated TypeScript to authored component positions.
      const compiler=dependency(request,'svelte/compiler');
      const {parse}=compiler.require(compiler.file);
      const member=request.operation.split('.').at(-1);
      const references=[],definitions=[];let tested=0,unresolved=0;
      const deadline=Date.now()+Math.min(request.timeoutMs,20000);
      for(const component of components) {
        if(Date.now()>deadline||tested>=200){unresolved++;break;}
        let ast;
        try{ast=parse(read(component),{modern:true});}catch{unresolved++;continue;}
        const candidates=[];
        function visit(node) {
          if(!node||typeof node!=='object')return;
          if(node.type==='MemberExpression' && ((node.property?.type==='Identifier'&&node.property.name===member)||((node.property?.type==='Literal'||node.property?.type==='StringLiteral')&&node.property.value===member)))candidates.push(node.property.start+(node.property.type==='Identifier'?0:1));
          if(node.type==='Property' && ((node.key?.type==='Identifier'&&node.key.name===member)||((node.key?.type==='Literal'||node.key?.type==='StringLiteral')&&node.key.value===member)))candidates.push(node.key.start+(node.key.type==='Identifier'?0:1));
          for(const [key,value] of Object.entries(node)){if(key==='parent'||key==='metadata')continue;if(Array.isArray(value))value.forEach(visit);else if(value&&typeof value==='object')visit(value);}
        }
        visit(ast.instance);visit(ast.module);visit(ast.fragment);
        for(const positionOffset of candidates) {
          if(Date.now()>deadline||tested++>=200){unresolved++;break;}
          const text=read(component),prefix=text.slice(0,positionOffset).split('\n'),params={textDocument:{uri:pathToFileURL(component).href},position:{line:prefix.length-1,character:prefix.at(-1).length}};
          const targets=lspLocations(request.project,await lsp.request('textDocument/definition',params),'svelte_language_server');
          const binding=bindings.find(binding=>targets.some(target=>target.file===binding.file&&target.line===binding.line&&target.column===binding.column));
          if(!binding)continue;
          definitions.push(...targets.map(target=>({...target,binding})));
          references.push(...lspLocations(request.project,await lsp.request('textDocument/references',{...params,context:{includeDeclaration:false}}),'svelte_language_server').map(reference=>({...reference,binding})));
        }
      }
      return {references,definitions,indexedComponents:components.length,complete:components.length===files.filter(source=>source.endsWith('.svelte')).length&&unresolved===0};
    }
    const text=read(file),prefix=text.slice(0,offset).split('\n');
    const params={textDocument:{uri:pathToFileURL(file).href},position:{line:prefix.length-1,character:prefix.at(-1).length}};
    if(!file.endsWith('.svelte'))return {references:[],definitions:[],complete:true};
    lsp.notify('textDocument/didOpen',{textDocument:{uri:params.textDocument.uri,languageId:'svelte',version:1,text}});
    const references=lspLocations(request.project,await lsp.request('textDocument/references',{...params,context:{includeDeclaration:false}}),'svelte_language_server');
    const definitions=action==='query'?lspLocations(request.project,await lsp.request('textDocument/definition',params),'svelte_language_server'):[];
    return {references,definitions};
  } finally {await lsp.close();}
}
function rustAnalyzer(request) {
  const candidates=['rust-analyzer'];
  for(const base of [process.env.LOCALAPPDATA,process.env.APPDATA].filter(Boolean)) {
    const root=path.join(base,'Zed/languages/rust-analyzer');
    function walk(directory,depth=0){if(depth>4||!fs.existsSync(directory))return;for(const entry of fs.readdirSync(directory,{withFileTypes:true})){const file=path.join(directory,entry.name);if(entry.isDirectory())walk(file,depth+1);else if(/^rust-analyzer(?:\.exe)?$/.test(entry.name))candidates.push(file);}}
    walk(root);
  }
  for(const binary of candidates)try{execFileSync(binary,['--version'],{stdio:'ignore',timeout:5000,windowsHide:true});return binary;}catch{}
  throw new Error('rust-analyzer is unavailable; install it with rustup component add rust-analyzer');
}
async function rustEvidence(request,file,action) {
  const root=request.project,manifests=['native/Cargo.toml','.revenant/desktop/Cargo.toml'].map(file=>path.join(root,file)).filter(file=>fs.existsSync(file));
  const lsp=new Lsp(rustAnalyzer(request),[],root,Math.min(request.timeoutMs,15000));
  try {
    await initialize(lsp,root,{linkedProjects:manifests,checkOnSave:false,check:{enable:false},cargo:{buildScripts:{enable:false},targetDir:path.join(root,'.revenant/target')},procMacro:{enable:false}});
    const text=read(file),uri=pathToFileURL(file).href;
    lsp.notify('textDocument/didOpen',{textDocument:{uri,languageId:'rust',version:1,text}});
    const position={line:request.line-1,character:request.column-1};
    let definitions=[],references=[];
    const until=Date.now()+Math.min(request.timeoutMs,20000);
    do {
      if(action==='symbols') {
        const symbols=(await lsp.request('workspace/symbol',{query:request.symbol??''})??[]).slice(0,request.maxResults).flatMap(symbol=>lspLocations(root,symbol.location,'rust_analyzer').map(location=>({name:symbol.name,kind:symbol.kind,...location})));
        if(symbols.length||lsp.status?.quiescent||Date.now()>=until)return {symbols,serverStatus:lsp.status};
        await pause(250);continue;
      }
      definitions=lspLocations(root,await lsp.request('textDocument/definition',{textDocument:{uri},position}),'rust_analyzer');
      references=lspLocations(root,await lsp.request('textDocument/references',{textDocument:{uri},position,context:{includeDeclaration:false}}),'rust_analyzer');
      if(definitions.length||references.length||lsp.status?.quiescent)break;
      await pause(250);
    } while(Date.now()<until);
    return {definitions,references,serverStatus:lsp.status};
  } finally {await lsp.close();}
}

export async function inspectSemantic(input) {
  const request={maxResults:100,timeoutMs:20000,...input,project:canonical(input.project)};
  if(!Number.isInteger(request.maxResults)||request.maxResults<1||request.maxResults>200)throw new Error('maxResults must be between 1 and 200');
  const scanned=sourceFiles(request.project),result={status:'complete',services:[],issues:[],references:[],definitions:[],diagnostics:[],sourceScan:{visitedEntries:scanned.visitedEntries,indexedFiles:scanned.files.length},unresolved:[{kind:'dynamic_reference',reason:'Computed property names, reflection and runtime-selected operation IDs are not established by static language services.'}],truncated:scanned.truncated};
  if(scanned.truncated){result.status='partial';result.issues.push({service:'index',message:'Source indexing reached its file or byte budget'});}
  let types;
  try {
    if(request.action==='query' && (request.file?.endsWith('.rs')||request.symbol)) {
      const file=allowed(request.project,request.file??'native/src/lib.rs');
      Object.assign(result,await rustEvidence(request,file,request.symbol?'symbols':'query'));
      result.services.push({name:'rust_analyzer',status:'partial',macroExpansion:'disabled',buildScripts:'disabled'});
      result.status='partial';
      result.issues.push({service:'rust_analyzer',message:'Definitions and references use the real analyzer. Build scripts and procedural macro expansion are disabled for bounded source-only indexing; compiled operation metadata remains authoritative for macro-generated contracts.'});
    } else {
      types=typeService(request,scanned.files);
      result.services.push({name:'typescript',status:'ready',version:types.ts.version});
      let file,offset,bindings=[];
      if(request.action==='operation') {
        file=allowed(request.project,request.facade);
        const positions=bindingPositions(types.ts,file,request.operation);
        if(!positions.length)throw new Error('Operation binding is absent in the current generated TypeScript facade');
        if(positions.length>request.maxResults){result.truncated=true;result.status='partial';result.issues.push({service:'typescript',message:'Generated operation declarations reached the result budget'});}
        bindings=positions.slice(0,request.maxResults).map(binding=>({...location(request.project,file,binding.text,binding.offset,binding.length,'compiled_typescript'),owner:binding.owner,symbolPath:binding.symbolPath,offset:binding.offset}));
        offset=bindings[0].offset;result.bindings=bindings.map(({offset,...binding})=>binding);result.binding=result.bindings[0];
      } else if(request.action==='query') {
        file=allowed(request.project,request.file);offset=offsetAt(read(file),request.line,request.column);
      }
      if(request.action==='diagnostics') {
        result.diagnostics.push(...types.configDiagnostics.map(item=>diagnostic(request.project,types.ts,item)));
        for(const file of scanned.files.filter(file=>/\.(?:ts|tsx|js|jsx)$/.test(file)))result.diagnostics.push(...types.service.getSyntacticDiagnostics(file).concat(types.service.getSemanticDiagnostics(file)).map(item=>diagnostic(request.project,types.ts,item)));
      } else if(!file.endsWith('.svelte')) {
        for(const selected of bindings.length?bindings:[{offset}]) {
          const {offset:position,...binding}=selected;
          const refs=types.service.findReferences(file,position)??[];
          const evidence=bindings.length?{binding}:{};
          result.references.push(...refs.flatMap(group=>group.references.filter(ref=>!ref.isDefinition).flatMap(ref=>inside(request.project,ref.fileName)?[{...location(request.project,ref.fileName,read(ref.fileName),ref.textSpan.start,ref.textSpan.length,'typescript_language_service'),...evidence}]:[])));
          result.definitions.push(...(types.service.getDefinitionAtPosition(file,position)??[]).flatMap(ref=>inside(request.project,ref.fileName)?[{...location(request.project,ref.fileName,read(ref.fileName),ref.textSpan.start,ref.textSpan.length,'typescript_language_service'),...evidence}]:[]));
        }
      }
      if(scanned.files.some(file=>file.endsWith('.svelte'))) {
        try{const svelte=await svelteEvidence(request,scanned.files,file,offset,request.action,result.bindings);result.services.push({name:'svelte_language_server',status:'ready',indexedComponents:svelte.indexedComponents});result.references.push(...svelte.references??[]);result.definitions.push(...svelte.definitions??[]);result.diagnostics.push(...svelte.diagnostics??[]);if(svelte.complete===false){result.status='partial';result.issues.push({service:'svelte_language_server',message:'Component indexing reached its budget or a component could not be parsed'});}}catch(error){result.status='partial';result.issues.push({service:'svelte_language_server',message:error.message});}
      }
    }
  } catch(error) {result.status=result.services.length?'partial':'unavailable';result.issues.push({service:request.file?.endsWith('.rs')?'rust_analyzer':'typescript',message:error.message});}
  finally {types?.service.dispose();}
  for(const field of ['references','definitions','diagnostics','symbols'])if(result[field]) {
    const entries=[...new Map(result[field].map(entry=>[`${entry.file}:${entry.line}:${entry.column}:${entry.code??''}`,entry])).values()];
    if(entries.length>request.maxResults)result.truncated=true;
    result[field]=entries.slice(0,request.maxResults);
  }
  return result;
}

if(process.argv.includes('--stdio') || process.argv.includes('--request-env')) {
  let buffer=process.argv.includes('--request-env') ? process.env.REVENANT_INSPECT_REQUEST??'' : '';
  if(!process.argv.includes('--request-env'))for await(const chunk of process.stdin){buffer+=chunk;if(buffer.length>1024*1024)throw new Error('Inspection request exceeds budget');}
  if(buffer.length>1024*1024)throw new Error('Inspection request exceeds budget');
  try {const result=await inspectSemantic(JSON.parse(buffer));process.stdout.write(JSON.stringify(result));}
  catch(error){process.stdout.write(JSON.stringify({status:'unavailable',issues:[{service:'inspection',message:error.message}],references:[],definitions:[],diagnostics:[]}));process.exitCode=1;}
}
