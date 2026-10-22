//! Resolve required TypeScript API exports from the package's declared entries.
use super::COMPATIBILITY_REVISION;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub(super) fn check(client: &Path) -> Result<()> {
    let metadata: Value = serde_json::from_slice(&fs::read(client.join("package.json"))?)?;
    ensure!(
        metadata["name"] == "@revenant/client" && metadata["version"] == "0.4.0",
        "expected @revenant/client 0.4.0"
    );
    ensure!(
        metadata["revenant"]["compatibility"].as_u64() == Some(u64::from(COMPATIBILITY_REVISION)),
        "client compatibility revision is missing or unsupported"
    );
    ensure!(
        metadata["exports"]["./vite"]["default"] == "./vite.js"
            && client.join("vite.js").is_file()
            && client.join("vite.d.ts").is_file(),
        "client is missing its development cache integration; run `revenant sdk upgrade`"
    );
    for (entry, values, types) in [
        (
            ".",
            &[
                "createApp",
                "App",
                "Operation",
                "Task",
                "callableOperation",
                "askSaveChanges",
                "onDesktopCloseRequested",
            ][..],
            &["ContractManifest", "OperationUpdates"][..],
        ),
        (
            "./svelte",
            &["setupApp", "provideApp", "useApp"][..],
            &[][..],
        ),
    ] {
        let declaration = &metadata["exports"][entry];
        let path = declaration
            .as_str()
            .or_else(|| declaration["default"].as_str())
            .with_context(|| format!("missing public entry point {entry}"))?;
        let exports = module_exports(client, &client.join(path), &mut BTreeSet::new())?;
        for symbol in values {
            ensure!(
                exports.values.contains(*symbol),
                "public client entry {entry} is missing runtime export {symbol}"
            );
        }
        for symbol in types {
            ensure!(
                exports.types.contains(*symbol),
                "public client entry {entry} is missing type export {symbol}"
            );
        }
    }
    Ok(())
}

// Comments and quoted examples cannot make a stale package appear compatible.
// This resolves declared local reexports; native/frontend compilation remains
// authoritative for implementation bodies and complete type correctness.
#[derive(Clone, Debug)]
enum Token {
    Word(String),
    Text(String),
    Literal,
    Punctuation(char),
}

fn starts_regex(input: &[Token]) -> bool {
    match input.last() {
        None => true,
        Some(Token::Word(value)) => {
            !matches!(
                input.get(input.len().saturating_sub(2)),
                Some(Token::Punctuation('.'))
            ) && [
                "return",
                "throw",
                "case",
                "delete",
                "typeof",
                "void",
                "new",
                "in",
                "of",
                "instanceof",
                "yield",
                "await",
                "default",
            ]
            .contains(&value.as_str())
        }
        Some(Token::Punctuation(
            '(' | '[' | '{' | '=' | ';' | ':' | ',' | '!' | '~' | '?' | '+' | '-' | '*' | '/' | '%'
            | '&' | '|' | '^' | '<' | '>',
        )) => true,
        Some(Token::Punctuation(')')) => {
            // A control condition starts a statement; a function call or grouped
            // value ends an expression and may instead be followed by division.
            let mut depth = 0usize;
            for (i, token) in input.iter().enumerate().rev() {
                match token {
                    Token::Punctuation(')') => depth += 1,
                    Token::Punctuation('(') => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return i.checked_sub(1).is_some_and(|i| matches!(&input[i], Token::Word(value) if ["if", "while", "for", "with", "switch", "catch"].contains(&value.as_str())));
                        }
                    }
                    _ => {}
                }
            }
            false
        }
        Some(Token::Punctuation('}')) => {
            // Declarations/control blocks end statements, while an object or
            // function expression may be divided by a following operand.
            let mut depth = 0usize;
            for (i, token) in input.iter().enumerate().rev() {
                match token {
                    Token::Punctuation('}') => depth += 1,
                    Token::Punctuation('{') => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            let start = input[..i]
                                .iter()
                                .rposition(|token| matches!(token, Token::Punctuation(';' | '}')))
                                .map_or(0, |i| i + 1);
                            let mut prefix = input[start..i].iter();
                            let mut first = prefix.next();
                            while matches!(first, Some(Token::Word(value)) if ["export", "default", "async", "declare", "abstract"].contains(&value.as_str()))
                            {
                                first = prefix.next();
                            }
                            return first.is_none()
                                || matches!(first, Some(Token::Word(value)) if ["function", "class", "interface", "type", "enum", "namespace", "if", "else", "while", "for", "try", "catch", "finally", "do", "switch"].contains(&value.as_str()));
                        }
                    }
                    _ => {}
                }
            }
            false
        }
        _ => false,
    }
}

// A template is one opaque value. Interpolations still need lexical scanning
// to find their closing brace; nested strings/comments/regex may contain braces
// and nested templates may contain their own interpolation delimiters.
fn skip_template(chars: &[char], i: &mut usize) {
    *i += 1;
    while *i < chars.len() {
        match chars[*i] {
            '\\' => *i = (*i + 2).min(chars.len()),
            '`' => {
                *i += 1;
                return;
            }
            '$' if chars.get(*i + 1) == Some(&'{') => {
                *i += 2;
                let mut depth = 1usize;
                let mut expression = Vec::new();
                while let Some(token) = next_token(chars, i, &expression) {
                    match token {
                        Token::Punctuation('{') => depth += 1,
                        Token::Punctuation('}') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    expression.push(token);
                }
            }
            _ => *i += 1,
        }
    }
}

fn next_token(chars: &[char], position: &mut usize, previous: &[Token]) -> Option<Token> {
    let mut i = *position;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }
        if c == '/' && starts_regex(previous) {
            i += 1;
            let mut character_class = false;
            while i < chars.len() && !matches!(chars[i], '\n' | '\r') {
                match chars[i] {
                    '\\' => i = (i + 2).min(chars.len()),
                    '[' => {
                        character_class = true;
                        i += 1;
                    }
                    ']' => {
                        character_class = false;
                        i += 1;
                    }
                    '/' if !character_class => {
                        i += 1;
                        while i < chars.len() && chars[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                        break;
                    }
                    _ => i += 1,
                }
            }
            *position = i;
            return Some(Token::Literal);
        }
        if c == '`' {
            skip_template(chars, &mut i);
            *position = i;
            return Some(Token::Literal);
        }
        if matches!(c, '\'' | '"') {
            let quote = c;
            let mut value = String::new();
            i += 1;
            while i < chars.len() && chars[i] != quote {
                if chars[i] == '\\' {
                    i += 1;
                }
                if i < chars.len() {
                    value.push(chars[i]);
                    i += 1;
                }
            }
            i += 1;
            *position = i;
            return Some(Token::Text(value));
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            *position = i;
            return Some(Token::Word(chars[start..i].iter().collect()));
        }
        // Only adjacent ++/-- can end an operand. Spaced `value + + /re/`
        // contains binary/unary operators and must still recognize the regex.
        if matches!(c, '+' | '-') && chars.get(i + 1) == Some(&c) && !starts_regex(previous) {
            *position = i + 2;
            return Some(Token::Literal);
        }
        // TS non-null assertions end the preceding expression, unlike unary
        // negation. Preserve !=/!== as operators that admit a regex operand.
        if c == '!' && chars.get(i + 1) != Some(&'=') && !starts_regex(previous) {
            *position = i + 1;
            return Some(Token::Literal);
        }
        *position = i + 1;
        return Some(Token::Punctuation(c));
    }
    *position = i;
    None
}

fn tokens(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut result = Vec::new();
    let mut i = 0;
    while let Some(token) = next_token(&chars, &mut i, &result) {
        result.push(token);
    }
    result
}

fn word(token: Option<&Token>, expected: &str) -> bool {
    matches!(token, Some(Token::Word(value)) if value == expected)
}

fn local_module(client: &Path, parent: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(
        relative.starts_with('.'),
        "client API cannot be resolved through external reexport {relative}"
    );
    let path = parent.join(relative);
    let path = if path.is_file() {
        path
    } else {
        path.with_extension("ts")
    };
    let resolved = dunce::canonicalize(path)?;
    ensure!(
        resolved.starts_with(dunce::canonicalize(client)?),
        "client entry point escapes package"
    );
    Ok(resolved)
}

#[derive(Default)]
struct Exports {
    values: BTreeSet<String>,
    types: BTreeSet<String>,
}
impl Exports {
    fn transfer(&mut self, available: &Self, original: &str, public: &str, type_only: bool) {
        if !type_only && available.values.contains(original) {
            self.values.insert(public.into());
        }
        if available.types.contains(original) || type_only && available.values.contains(original) {
            self.types.insert(public.into());
        }
    }
    fn declaration(&mut self, kind: &str, name: &str) {
        if matches!(
            kind,
            "function" | "class" | "const" | "let" | "var" | "enum"
        ) {
            self.values.insert(name.into());
        }
        if matches!(kind, "class" | "interface" | "type" | "enum") {
            self.types.insert(name.into());
        }
    }
}

fn named_clause(input: &[Token], i: &mut usize, type_only: bool) -> Vec<(String, String, bool)> {
    *i += 1;
    let mut names = Vec::new();
    while *i < input.len() && !matches!(input[*i], Token::Punctuation('}')) {
        let named_type = word(input.get(*i), "type") && !word(input.get(*i + 1), "as");
        if named_type {
            *i += 1;
        }
        if let Some(Token::Word(name)) = input.get(*i) {
            let original = name.clone();
            let mut public = original.clone();
            *i += 1;
            if word(input.get(*i), "as") {
                if let Some(Token::Word(alias)) = input.get(*i + 1) {
                    public = alias.clone();
                }
                *i += 2;
            }
            names.push((original, public, type_only || named_type));
        } else {
            *i += 1;
        }
    }
    *i += 1;
    names
}

fn declaration_names(input: &[Token], start: usize) -> Vec<String> {
    let Some(Token::Word(kind)) = input.get(start) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let name_index = start
        + 1
        + usize::from(
            kind == "function" && matches!(input.get(start + 1), Some(Token::Punctuation('*'))),
        );
    if let Some(Token::Word(name)) = input.get(name_index) {
        names.push(name.clone());
    }
    if !["const", "let", "var"].contains(&kind.as_str()) {
        return names;
    }
    let mut depth = 0usize;
    for i in start + 2..input.len() {
        // Preserve ASI boundaries: a later, unexported declaration cannot lend
        // its bindings to the preceding export when semicolons are omitted.
        if depth == 0
            && matches!(&input[i], Token::Word(value) if ["export", "const", "let", "var"].contains(&value.as_str()))
            && !matches!(input.get(i - 1), Some(Token::Punctuation('.')))
            && !word(input.get(i - 1), "as")
        {
            break;
        }
        match input[i] {
            Token::Punctuation('(' | '[' | '{') => depth += 1,
            Token::Punctuation(')' | ']' | '}') => depth = depth.saturating_sub(1),
            Token::Punctuation(';') if depth == 0 => break,
            Token::Punctuation(',') if depth == 0 => {
                // Commas in a TS generic type are not additional declarators.
                if let Some(Token::Word(name)) = input.get(i + 1)
                    && (matches!(
                        input.get(i + 2),
                        Some(Token::Punctuation('=' | ':' | '!' | ';' | ','))
                    ) || input.get(i + 2).is_none())
                {
                    names.push(name.clone());
                }
            }
            _ => {}
        }
    }
    names
}

fn declaration_position(input: &[Token], index: usize) -> bool {
    let mut start = index;
    while start > 0
        && matches!(&input[start - 1], Token::Word(value) if ["export", "default", "async", "abstract", "declare"].contains(&value.as_str()))
    {
        start -= 1;
    }
    // Expression-only positions do not introduce module bindings. In particular,
    // the name of `const value = async function name() {}` is scoped to the
    // function expression, and cannot satisfy a later `export { name }`.
    match start.checked_sub(1).and_then(|i| input.get(i)) {
        None => true,
        Some(Token::Word(value)) => ![
            "return",
            "throw",
            "case",
            "delete",
            "typeof",
            "void",
            "new",
            "yield",
            "await",
            "in",
            "of",
            "instanceof",
            "as",
            "satisfies",
        ]
        .contains(&value.as_str()),
        Some(Token::Literal | Token::Text(_) | Token::Punctuation(';' | '}' | ')' | ']')) => true,
        // Numeric literal tokens also allow the following ASI declaration.
        Some(Token::Punctuation(value)) => value.is_ascii_digit(),
    }
}

fn bindings(
    client: &Path,
    path: &Path,
    input: &[Token],
    visited: &mut BTreeSet<PathBuf>,
) -> Result<Exports> {
    let mut local = Exports::default();
    let mut depth = 0usize;
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            Token::Punctuation('{' | '(' | '[') => depth += 1,
            Token::Punctuation('}' | ')' | ']') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 {
            if let Some(Token::Word(kind)) = input.get(i)
                && declaration_position(input, i)
                && !word(i.checked_sub(1).and_then(|j| input.get(j)), "declare")
                && !(word(i.checked_sub(1).and_then(|j| input.get(j)), "abstract")
                    && word(i.checked_sub(2).and_then(|j| input.get(j)), "declare"))
                && !(kind == "enum" && word(i.checked_sub(1).and_then(|j| input.get(j)), "const"))
            {
                for name in declaration_names(input, i) {
                    local.declaration(kind, &name);
                }
            }
            if word(input.get(i), "import") {
                i += 1;
                let type_only = word(input.get(i), "type");
                if type_only {
                    i += 1;
                }
                let mut names = Vec::new();
                if let Some(Token::Word(name)) = input.get(i) {
                    names.push(("default".into(), name.clone(), type_only));
                    i += 1;
                    if matches!(input.get(i), Some(Token::Punctuation(','))) {
                        i += 1;
                    }
                }
                if matches!(input.get(i), Some(Token::Punctuation('{'))) {
                    names.extend(named_clause(input, &mut i, type_only));
                }
                if word(input.get(i), "from")
                    && let Some(Token::Text(relative)) = input.get(i + 1)
                    && relative.starts_with('.')
                {
                    let target =
                        local_module(client, path.parent().context("module parent")?, relative)?;
                    let available = module_exports(client, &target, visited)?;
                    for (original, public, type_only) in names {
                        local.transfer(&available, &original, &public, type_only);
                    }
                }
            }
        }
        i += 1;
    }
    Ok(local)
}

fn module_exports(client: &Path, path: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<Exports> {
    let path = dunce::canonicalize(path)?;
    ensure!(
        path.starts_with(dunce::canonicalize(client)?),
        "client entry point escapes package"
    );
    if !visited.insert(path.clone()) {
        return Ok(Exports::default());
    }
    let input = tokens(&fs::read_to_string(&path)?);
    let local = bindings(client, &path, &input, visited)?;
    let mut exports = Exports::default();
    let mut i = 0;
    let mut depth = 0usize;
    while i < input.len() {
        match input[i] {
            Token::Punctuation('{') => depth += 1,
            Token::Punctuation('}') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth != 0 || !word(input.get(i), "export") {
            i += 1;
            continue;
        }
        i += 1;
        let type_only = word(input.get(i), "type")
            && matches!(input.get(i + 1), Some(Token::Punctuation('{' | '*')));
        if type_only {
            i += 1;
        }
        if matches!(input.get(i), Some(Token::Punctuation('*'))) {
            i += 1;
            if word(input.get(i), "from")
                && let Some(Token::Text(relative)) = input.get(i + 1)
            {
                let module =
                    local_module(client, path.parent().context("module parent")?, relative)?;
                let available = module_exports(client, &module, visited)?;
                if !type_only {
                    exports.values.extend(
                        available
                            .values
                            .into_iter()
                            .filter(|name| name != "default"),
                    );
                }
                exports
                    .types
                    .extend(available.types.into_iter().filter(|name| name != "default"));
            }
        } else if matches!(input.get(i), Some(Token::Punctuation('{'))) {
            let names = named_clause(&input, &mut i, type_only);
            if word(input.get(i), "from")
                && let Some(Token::Text(relative)) = input.get(i + 1)
            {
                let module =
                    local_module(client, path.parent().context("module parent")?, relative)?;
                let available = module_exports(client, &module, visited)?;
                for (original, public, type_only) in names {
                    exports.transfer(&available, &original, &public, type_only);
                }
            } else {
                for (original, public, type_only) in names {
                    exports.transfer(&local, &original, &public, type_only);
                }
            }
        } else {
            let default = word(input.get(i), "default");
            if default {
                i += 1;
            }
            if word(input.get(i), "async") {
                i += 1;
            }
            if [
                "function",
                "class",
                "interface",
                "type",
                "const",
                "let",
                "var",
                "enum",
            ]
            .iter()
            .any(|keyword| word(input.get(i), keyword))
                && !declaration_names(&input, i).is_empty()
            {
                if let Some(Token::Word(kind)) = input.get(i) {
                    for name in declaration_names(&input, i) {
                        exports.declaration(kind, if default { "default" } else { &name });
                    }
                }
            } else if default {
                if word(input.get(i), "function") || word(input.get(i), "class") {
                    let kind = if word(input.get(i), "class") {
                        "class"
                    } else {
                        "function"
                    };
                    exports.declaration(kind, "default");
                } else if let Some(Token::Word(name)) = input.get(i) {
                    exports.transfer(&local, name, "default", false);
                } else {
                    // Default arrow/function expressions are runtime values.
                    exports.values.insert("default".into());
                }
            }
        }
        i += 1;
    }
    // The traversal set is a recursion guard, not a module cache: two different
    // reexport clauses may legitimately reference the same module.
    visited.remove(&path);
    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comments_and_string_literals_do_not_supply_exports() {
        let tokens = tokens(
            "// export function setupApp() {}\n'export function setupApp() {}'; /* export class App {} */ export function useApp() {}",
        );
        assert_eq!(
            tokens
                .iter()
                .filter(|token| word(Some(token), "export"))
                .count(),
            1
        );
    }

    #[test]
    fn runtime_requirements_reject_interfaces_and_type_only_reexports() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        for declaration in [
            "export interface setupApp {}",
            "export type setupApp = () => void;",
            "export type { setupApp } from './helpers.ts';",
            "export { type setupApp } from './helpers.ts';",
        ] {
            fs::write(
                client.join("src/helpers.ts"),
                "export function setupApp() {}\n",
            )
            .unwrap();
            fs::write(client.join("src/svelte.ts"), format!("{declaration}\nexport function provideApp() {{}}\nexport function useApp() {{}}\n")).unwrap();
            assert!(
                format!("{:#}", check(&client).unwrap_err()).contains("runtime export setupApp"),
                "accepted {declaration}"
            );
        }
    }

    #[test]
    fn compatible_default_value_reexports_and_local_import_aliases_are_supported() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        fs::write(
            client.join("src/helper.ts"),
            "export default function configure() {}\n",
        )
        .unwrap();
        fs::write(client.join("src/svelte.ts"), "export { default as setupApp } from './helper.js';\nimport helper from './helper.js';\nexport { helper as provideApp };\nexport function useApp() {}\n").unwrap();
        check(&client).unwrap();
    }

    #[test]
    fn regular_expression_contents_do_not_supply_runtime_exports() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        for expression in [
            r"const example = /export function setupApp/;",
            r"const example = /[\/]export function setupApp/gu;",
            r"const example = /\/export function setupApp\//;",
            r"const ratio = 6 / 2; const example = /export function setupApp/;",
            r"if (true) /export function setupApp/.test('example');",
            r"export {} /export function setupApp/.test('example');",
            r"function example() {} /export function setupApp/.test('example');",
            r"const value = 1; const example = value + + /export function setupApp/;",
            r"const value = 1; const example = value - - /export function setupApp/;",
            r"const example = ! /export function setupApp/;",
            r"const value = 1; const example = value != /export function setupApp/;",
            r"const value = 1; const example = value !== /export function setupApp/;",
            r"const example = 6 / /export function setupApp/;",
        ] {
            fs::write(client.join("src/svelte.ts"), format!("{expression}\nexport function provideApp() {{}}\nexport function useApp() {{}}\n")).unwrap();
            assert!(
                format!("{:#}", check(&client).unwrap_err()).contains("runtime export setupApp"),
                "accepted regex: {expression}"
            );
        }
    }

    #[test]
    fn nested_templates_and_interpolations_are_opaque_to_runtime_export_checks() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        for expression in [
            r"const text = `example ${`export function setupApp`}`;",
            r"const text = `example ${`nested ${`export function setupApp`}`}`;",
            r#"const text = `example ${ { message: "}", other: '}', nested: `export function setupApp` } }`;"#,
            r"const text = `example ${ /* } ` export function setupApp */ `nested` }`;",
            "const text = `example ${ // } ` export function setupApp\n `nested` }`;",
            r"const text = `example ${ /[}`]/.test('}') ? `export function setupApp` : '' }`;",
            r"const text = `example ${ /\}/.test('}') ? `export function setupApp` : '' }`;",
            r"const text = `example ${ (() => { const message = `export function setupApp`; return message; })() }`;",
            r"const text = `example ${ { return: 6 }.return / 2 }`;",
            r"let counter = 6; const text = `example ${ counter++ / 2 }`;",
            r"const text = `escaped \` export function setupApp \${ignored}`;",
        ] {
            let remaining = "export function provideApp() {}\nexport function useApp() {}\n";
            fs::write(
                client.join("src/svelte.ts"),
                format!("{expression}\n{remaining}"),
            )
            .unwrap();
            assert!(
                format!("{:#}", check(&client).unwrap_err()).contains("runtime export setupApp"),
                "accepted template contents: {expression}"
            );
            // Template scanning must also stop at the true delimiter and retain
            // the following real declaration, rather than swallowing the file.
            fs::write(
                client.join("src/svelte.ts"),
                format!("{expression}\nexport function setupApp() {{}}\n{remaining}"),
            )
            .unwrap();
            check(&client).unwrap_or_else(|error| {
                panic!("lost genuine exports after {expression}: {error:#}")
            });
        }
    }

    #[test]
    fn division_after_properties_and_postfix_updates_preserves_real_exports() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        for expression in [
            "const obj = { return: 6 }; const ratio = obj.return / 2;",
            "let counter = 6; const ratio = counter++ / 2;",
            "let counter = 6; const ratio = counter-- / 2;",
            "const obj = { throw: 6 }; const ratio = obj?.throw / 2;",
            "const value: number | undefined = 6; const ratio = value! / 2;",
            "const value: number | undefined = 6; const ratio = (value)! / 2;",
            "const value: number[] | undefined = [6]; const ratio = value![0]! / 2;",
            "const ratio = 6! / 2;",
        ] {
            fs::write(client.join("src/svelte.ts"), format!("{expression} export function setupApp() {{}} export function provideApp() {{}} export function useApp() {{}}")).unwrap();
            check(&client).unwrap_or_else(|error| {
                panic!("division swallowed exports after {expression}: {error:#}")
            });
        }
    }

    #[test]
    fn multiple_variable_exports_and_local_aliases_are_supported() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        for declaration in [
            "export const setupApp = () => {}, provideApp = () => {}, useApp = () => {};",
            "const setupApp = () => {}, provideApp = () => {}, useApp = () => {}; export { setupApp, provideApp, useApp };",
            "export const setupApp = function() {}, provideApp = function() {}, useApp = function() {};",
            "export const dummy: Record<string, number> = {}, setupApp = () => {}, provideApp = () => {}, useApp = () => {};",
            "export const dummy = [] as const, setupApp = () => {}, provideApp = () => {}, useApp = () => {};",
        ] {
            fs::write(client.join("src/svelte.ts"), declaration).unwrap();
            check(&client).unwrap_or_else(|error| {
                panic!("lost multiple exports in {declaration}: {error:#}")
            });
        }
        fs::write(client.join("src/svelte.ts"), "export const example = 0\nconst other = 0, setupApp = () => {}, provideApp = () => {}, useApp = () => {};\n").unwrap();
        assert!(format!("{:#}", check(&client).unwrap_err()).contains("runtime export setupApp"));
    }

    #[test]
    fn ambient_declarations_do_not_supply_runtime_exports() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        for declaration in [
            "export declare function setupApp(): void;",
            "declare function setupApp(): void; export { setupApp };",
            "export declare const setupApp: () => void;",
            "declare const setupApp: () => void; export { setupApp };",
            "declare const example: number, setupApp: () => void; export { setupApp };",
            "declare class setupApp {} export { setupApp };",
            "declare abstract class setupApp {} export { setupApp };",
            "declare const enum setupApp { Example } export { setupApp };",
        ] {
            fs::write(client.join("src/svelte.ts"), format!("{declaration}\nexport function provideApp() {{}}\nexport function useApp() {{}}\n")).unwrap();
            assert!(
                format!("{:#}", check(&client).unwrap_err()).contains("runtime export setupApp"),
                "accepted ambient declaration: {declaration}"
            );
        }
    }

    #[test]
    fn named_function_and_class_expressions_do_not_create_module_bindings() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        for expression in [
            "const unused = function setupApp() {};",
            "const unused = async function setupApp() {};",
            "const unused = function* setupApp() {};",
            "const unused = async function* setupApp() {};",
            "const unused = class setupApp {};",
            "const unused = (function setupApp() {});",
            "const unused = (async function setupApp() {});",
            "const unused = (function* setupApp() {});",
            "const unused = (async function* setupApp() {});",
            "const unused = (class setupApp {});",
            "const unused = true ? function setupApp() {} : () => {};",
            "const unused = void function setupApp() {};",
            "const unused = new class setupApp {};",
            "const unused = [function setupApp() {}];",
            "const unused = { method: function setupApp() {} };",
        ] {
            fs::write(client.join("src/svelte.ts"), format!("declare function setupApp(): void; {expression} export {{ setupApp }}; export function provideApp() {{}} export function useApp() {{}}")).unwrap();
            assert!(
                format!("{:#}", check(&client).unwrap_err()).contains("runtime export setupApp"),
                "accepted expression-local name: {expression}"
            );
        }
        for declaration in [
            "const unrelated = 1\nfunction setupApp() {} export { setupApp };",
            "const unrelated = () => {}\nasync function setupApp() {} export { setupApp };",
            "const setupApp = function internal() {}; export { setupApp };",
            "const setupApp = async function internal() {}; export { setupApp };",
        ] {
            fs::write(client.join("src/svelte.ts"), format!("{declaration}\nexport function provideApp() {{}}\nexport function useApp() {{}}\n")).unwrap();
            check(&client)
                .unwrap_or_else(|error| panic!("lost module binding in {declaration}: {error:#}"));
        }
    }

    #[test]
    fn generator_declarations_are_runtime_exports() {
        let temp = tempfile::TempDir::new().unwrap();
        super::super::materialize(temp.path()).unwrap();
        let client = temp.path().join(".revenant/sdk/packages/client");
        for declaration in [
            "function* setupApp() {} export { setupApp };",
            "async function* setupApp() {} export { setupApp };",
            "export function* setupApp() {}",
            "export async function* setupApp() {}",
            "export default function* setupApp() {} export { setupApp };",
            "export default async function* setupApp() {} export { setupApp };",
        ] {
            fs::write(client.join("src/svelte.ts"), format!("{declaration}\nexport function provideApp() {{}}\nexport function useApp() {{}}\n")).unwrap();
            check(&client).unwrap_or_else(|error| {
                panic!("lost generator export in {declaration}: {error:#}")
            });
        }
        // Required values can also reexport a named or anonymous default
        // generator without incorrectly exporting its expression-local name.
        for declaration in [
            "export default function* generator() {}",
            "export default function* () {}",
            "export default async function* generator() {}",
            "export default async function* () {}",
        ] {
            fs::write(client.join("src/helpers.ts"), declaration).unwrap();
            fs::write(client.join("src/svelte.ts"), "export { default as setupApp } from './helpers.ts';\nexport function provideApp() {}\nexport function useApp() {}\n").unwrap();
            check(&client).unwrap_or_else(|error| {
                panic!("lost default generator reexport in {declaration}: {error:#}")
            });
        }
    }
}
