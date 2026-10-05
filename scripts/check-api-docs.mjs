/** Check the client package's reachable exported declarations with its own TypeScript parser. */
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const client = path.join(root, 'packages/client');
const require = createRequire(path.join(client, 'package.json'));
let ts;
try {
  ts = require(path.join(client, 'node_modules/typescript'));
} catch {
  console.error('Install packages/client dependencies first: npm install --prefix packages/client');
  process.exit(1);
}
const metadata = JSON.parse(fs.readFileSync(path.join(client, 'package.json'), 'utf8'));
const entries = Object.entries(metadata.exports).map(([name, entry]) => {
  const target = typeof entry === 'string' ? entry : entry.types ?? entry.default;
  if (!target?.endsWith('.ts')) throw new Error(`No TypeScript entry for export ${name}`);
  return [name, path.resolve(client, target)];
});
const config = ts.readConfigFile(path.join(client, 'tsconfig.json'), ts.sys.readFile);
if (config.error) throw new Error(ts.flattenDiagnosticMessageText(config.error.messageText, '\n'));
const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, client);
if (parsed.errors.length) throw new Error(parsed.errors.map(error => ts.flattenDiagnosticMessageText(error.messageText, '\n')).join('\n'));
// This is a documentation gate. Typechecking remains a separate CI step.
const program = ts.createProgram([...new Set([...parsed.fileNames, ...entries.map(([, file]) => file)])], parsed.options);
const syntaxErrors = program.getSyntacticDiagnostics().filter(error => error.file?.fileName.startsWith(path.join(client, 'src')));
if (syntaxErrors.length) {
  console.error(ts.formatDiagnosticsWithColorAndContext(syntaxErrors, {
    getCanonicalFileName: file => file,
    getCurrentDirectory: () => root,
    getNewLine: () => '\n',
  }));
  process.exit(1);
}
const checker = program.getTypeChecker();
const visited = new Set();
const errors = new Set();
let checked = 0;
const local = node => node && path.relative(path.join(client, 'src'), node.getSourceFile().fileName).split(path.sep)[0] !== '..'
  && !node.getSourceFile().fileName.includes('node_modules');
const modifiers = node => ts.getCombinedModifierFlags(node);
const publicMember = node => !(modifiers(node) & (ts.ModifierFlags.Private | ts.ModifierFlags.Protected))
  && !(node.name && ts.isPrivateIdentifier(node.name));
const comments = node => {
  const docs = ts.getJSDocCommentsAndTags(node).filter(ts.isJSDoc);
  return docs.map(doc => typeof doc.comment === 'string' ? doc.comment : doc.comment?.map(part => part.text).join('') ?? '').join(' ').trim();
};
const readable = text => {
  const plain = text.replace(/\{@\w+\s+([^}]+)\}/g, '$1').replace(/<[^>]+>/g, '').trim();
  return (plain.match(/[A-Za-z][A-Za-z'-]*/g) ?? []).length >= 3
    && !/^(?:todo|tbd|fixme|documentation pending|description here)\b/i.test(plain);
};
function document(node, label, fallback) {
  const source = node.getSourceFile();
  if (/\@internal\b/.test(ts.getJSDocCommentsAndTags(node).map(doc => doc.getText(source)).join(' '))) {
    // Exporting an internal declaration is still a public API leak, not an exemption.
    errors.add(`${path.relative(root, source.fileName)}: exported ${label} is marked @internal`);
  }
  checked++;
  if (readable(comments(node)) || (fallback && readable(comments(fallback)))) return;
  const { line } = source.getLineAndCharacterOfPosition(node.getStart(source));
  errors.add(`${path.relative(root, source.fileName).replaceAll('\\', '/')}:${line + 1}: ${label} needs a readable JSDoc description`);
}
function visitType(node, label) {
  if (!node) return;
  const reference = ts.isTypeReferenceNode(node) ? node.typeName
    : ts.isExpressionWithTypeArguments(node) ? node.expression
      : ts.isTypeQueryNode(node) ? node.exprName : undefined;
  if (reference) {
    const symbol = checker.getSymbolAtLocation(reference);
    if (symbol) visitSymbol(symbol, reference.getText());
  }
  if (ts.isTypeLiteralNode(node)) {
    for (const member of node.members) visitDeclaration(member, `${label}.${member.name?.getText() ?? 'signature'}`);
    return;
  }
  ts.forEachChild(node, child => visitType(child, label));
}
function visitDeclaration(node, label) {
  if (!local(node) || visited.has(node) || !publicMember(node)) return;
  visited.add(node);
  const variableStatement = ts.isVariableDeclaration(node) ? node.parent.parent : undefined;
  document(node, label, variableStatement);
  if (ts.isClassDeclaration(node) || ts.isInterfaceDeclaration(node)) {
    for (const member of node.members) {
      if (!publicMember(member)) continue;
      visitDeclaration(member, `${label}.${member.name?.getText() ?? (ts.isConstructorDeclaration(member) ? 'constructor' : 'signature')}`);
      if (ts.isConstructorDeclaration(member)) {
        for (const parameter of member.parameters) {
          if (ts.isParameterPropertyDeclaration(parameter, member) && publicMember(parameter)) {
            visitDeclaration(parameter, `${label}.${parameter.name.getText()}`);
          }
        }
      }
    }
    for (const clause of node.heritageClauses ?? []) visitType(clause, label);
  }
  if (ts.isEnumDeclaration(node)) for (const member of node.members) visitDeclaration(member, `${label}.${member.name.getText()}`);
  if (ts.isModuleDeclaration(node)) {
    const symbol = checker.getSymbolAtLocation(node.name);
    if (symbol) for (const exported of checker.getExportsOfModule(symbol)) visitSymbol(exported, `${label}.${exported.name}`);
  }
  visitType(node.type, label);
  // Public inferred values can expose a local class/type even without an
  // annotation (for example a Readable signal property initialized with new).
  if (!node.type && (ts.isVariableDeclaration(node) || ts.isPropertyDeclaration(node))) {
    const type = checker.getTypeAtLocation(node);
    const symbol = type.aliasSymbol ?? type.getSymbol();
    if (symbol) visitSymbol(symbol, `${label} type`);
  }
  for (const parameter of node.parameters ?? []) visitType(parameter.type, label);
  for (const parameter of node.typeParameters ?? []) {
    visitType(parameter.constraint, label);
    visitType(parameter.default, label);
  }
}
function visitSymbol(symbol, label) {
  const target = symbol.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(symbol) : symbol;
  if (target.flags & ts.SymbolFlags.TypeParameter) return;
  if (!target.declarations?.length && (symbol.flags & ts.SymbolFlags.Alias)) {
    for (const declaration of symbol.declarations ?? []) {
      if (!local(declaration) || !ts.isExportSpecifier(declaration)) continue;
      const source = declaration.parent.parent.moduleSpecifier?.text;
      if (source?.startsWith('.') && !source.endsWith('.svelte')) {
        errors.add(`${path.relative(root, declaration.getSourceFile().fileName)}: cannot resolve public export ${label}`);
      }
    }
  }
  for (const declaration of target.declarations ?? []) visitDeclaration(declaration, label);
}
for (const [entry, file] of entries) {
  const source = program.getSourceFile(file);
  const symbol = source && checker.getSymbolAtLocation(source);
  if (!symbol) throw new Error(`Cannot read public entry ${entry}: ${file}`);
  for (const exported of checker.getExportsOfModule(symbol)) visitSymbol(exported, `${entry}:${exported.name}`);
  // TypeScript has no declaration for an imported .svelte component here. Check
  // the public re-export itself instead of silently skipping that API.
  for (const statement of source.statements) {
    if (ts.isExportDeclaration(statement) && statement.moduleSpecifier?.text.endsWith('.svelte')) {
      document(statement, `${entry}:${statement.exportClause?.getText() ?? 'component'}`);
    }
  }
}
if (!checked) throw new Error('No public declarations checked');
if (errors.size) {
  console.error([...errors].sort().join('\n'));
  console.error(`API docs: ${errors.size} problem(s), ${checked} declarations checked.`);
  process.exitCode = 1;
} else {
  console.log(`API docs: ${checked} public declarations documented across ${entries.length} package exports.`);
}
