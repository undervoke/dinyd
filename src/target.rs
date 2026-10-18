//! Selector resolution and digests.

use std::ops::Range;

use sha2::{Digest as _, Sha256};
use tree_sitter::{Language, Node, Parser, Tree};

use crate::Error;
use crate::anchor::{Mode, Selector};
use crate::git::{Lookup, Snapshot};

/// Why a selector does not resolve to exactly one valid target.
#[derive(Debug)]
pub enum Failure {
    Unresolved(String),
    Read(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Failure {
        Failure::Read(e)
    }
}

fn unresolved<T>(reason: impl Into<String>) -> Result<T, Failure> {
    Err(Failure::Unresolved(reason.into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    CSharp,
    TypeScript,
    Tsx,
    JavaScript,
    Go,
    Json,
    Yaml,
    Css,
    Vue,
    Other,
}

impl Format {
    fn of(path: &str) -> Format {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("cs") => Format::CSharp,
            Some("ts" | "mts" | "cts") => Format::TypeScript,
            Some("tsx") => Format::Tsx,
            Some("js" | "mjs" | "cjs" | "jsx") => Format::JavaScript,
            Some("go") => Format::Go,
            Some("json") => Format::Json,
            Some("yaml" | "yml") => Format::Yaml,
            Some("css") => Format::Css,
            Some("vue") => Format::Vue,
            _ => Format::Other,
        }
    }

    /// Grammar for whole-file structural normalization; `None` means b1 is the default.
    fn language(self) -> Option<Language> {
        Some(match self {
            Format::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Format::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Format::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Format::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Format::Go => tree_sitter_go::LANGUAGE.into(),
            Format::Json => tree_sitter_json::LANGUAGE.into(),
            Format::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Format::Css => tree_sitter_css::LANGUAGE.into(),
            Format::Vue | Format::Other => return None,
        })
    }
}

/// A selected part of a file: its byte range and its structural token stream.
struct Region {
    range: Range<usize>,
    tokens: Vec<u8>,
}

/// Checks that a `dinyd-ref:` selector names exactly one existing target; directories allowed.
pub fn exists(snapshot: &Snapshot, selector: &Selector) -> Result<(), Failure> {
    match snapshot.lookup(&selector.path) {
        Lookup::Dir if selector.location.is_none() => Ok(()),
        Lookup::Dir => unresolved("a directory has no locations"),
        Lookup::File => match &selector.location {
            None => Ok(()),
            Some(location) => {
                let source = snapshot.read(&selector.path)?;
                regions(&selector.path, &source, location).map(|_| ())
            }
        },
        Lookup::Rejected => unresolved("not a regular file"),
        Lookup::Missing => unresolved("no such path"),
    }
}

/// Digest of a `dinyd:` target in `mode`, or in the format default when `mode` is `None`.
pub fn digest(
    snapshot: &Snapshot,
    selector: &Selector,
    mode: Option<Mode>,
) -> Result<(Mode, String), Failure> {
    match snapshot.lookup(&selector.path) {
        Lookup::File => {}
        Lookup::Dir => return unresolved("a digest anchor needs a file"),
        Lookup::Rejected => return unresolved("not a regular file"),
        Lookup::Missing => return unresolved("no such path"),
    }
    let source = snapshot.read(&selector.path)?;
    let format = Format::of(&selector.path);
    let regions = match &selector.location {
        Some(location) => regions(&selector.path, &source, location)?,
        None => {
            let wants_s1 = mode.map_or(format.language().is_some(), |m| m == Mode::S1);
            if !wants_s1 {
                return Ok((Mode::B1, hex(&Sha256::digest(crlf_to_lf(&source)))));
            }
            let Some(language) = format.language() else {
                return unresolved("s1 is not available for this format");
            };
            let tree = parse(language, &source)?;
            vec![region(tree.root_node(), &source, 0)]
        }
    };
    let mode = mode.unwrap_or(Mode::S1);
    // A name(*) group is one region: its members' contents in source order.
    let mut hasher = Sha256::new();
    for region in &regions {
        match mode {
            Mode::S1 => hasher.update(&region.tokens),
            Mode::B1 => hasher.update(crlf_to_lf(&source[region.range.clone()])),
        }
    }
    Ok((mode, hex(&hasher.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    bytes[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn crlf_to_lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn parse(language: Language, source: &[u8]) -> Result<Tree, Failure> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| Failure::Read(Error::Read(format!("grammar: {e}"))))?;
    match parser.parse(source, None) {
        Some(tree) if !tree.root_node().has_error() => Ok(tree),
        _ => unresolved("parse failed"),
    }
}

/// Region of `node`; `offset` maps an embedded source (a Vue script block) back to the file.
fn region(node: Node, source: &[u8], offset: usize) -> Region {
    let mut tokens = Vec::new();
    serialize(node, source, &mut tokens);
    Region {
        range: node.start_byte() + offset..node.end_byte() + offset,
        tokens,
    }
}

/// Structural token stream: tokens plus nesting markers per named node, without node kinds.
/// Whitespace between tokens never reaches the stream. Strings, scalars, and comments keep
/// their literal content verbatim, since some grammars (YAML) leave it outside any leaf.
fn serialize(node: Node, source: &[u8], out: &mut Vec<u8>) {
    let payload = node.is_named()
        && ["string", "scalar", "comment"]
            .iter()
            .any(|k| node.kind().contains(k));
    if node.child_count() == 0 {
        token(out, &source[node.byte_range()]);
        return;
    }
    if payload {
        literal(node, source, out);
        return;
    }
    if node.is_named() {
        out.push(b'(');
    }
    let mut at = node.start_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        gap(&source[at..child.start_byte().max(at)], out);
        serialize(child, source, out);
        at = child.end_byte();
    }
    gap(&source[at..node.end_byte().max(at)], out);
    if node.is_named() {
        out.push(b')');
    }
}

/// A literal is verbatim text, except embedded code (template substitutions, C# interpolations),
/// which is serialized like any other code.
fn literal(node: Node, source: &[u8], out: &mut Vec<u8>) {
    out.push(b'(');
    let mut at = node.start_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "template_substitution" | "interpolation") {
            if at < child.start_byte() {
                token(out, &source[at..child.start_byte()]);
            }
            serialize(child, source, out);
            at = child.end_byte();
        }
    }
    if at < node.end_byte() {
        token(out, &source[at..node.end_byte()]);
    }
    out.push(b')');
}

fn token(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'T');
    out.extend((bytes.len() as u32).to_le_bytes());
    out.extend(bytes);
}

/// Text a grammar leaves between children; only whitespace there is formatting.
fn gap(text: &[u8], out: &mut Vec<u8>) {
    let start = text.iter().position(|b| !b.is_ascii_whitespace());
    let end = text.iter().rposition(|b| !b.is_ascii_whitespace());
    if let (Some(start), Some(end)) = (start, end) {
        token(out, &text[start..=end]);
    }
}

fn regions(path: &str, source: &[u8], location: &str) -> Result<Vec<Region>, Failure> {
    let format = Format::of(path);
    match format {
        Format::CSharp | Format::TypeScript | Format::Tsx | Format::JavaScript | Format::Go => {
            let tree = parse(format.language().unwrap(), source)?;
            let mut symbols = Vec::new();
            collect(
                tree.root_node(),
                source,
                lang_of(format),
                &mut Vec::new(),
                &mut symbols,
            );
            pick_symbols(location, symbols, source, 0)
        }
        Format::Vue => vue_regions(source, location),
        Format::Json | Format::Yaml => {
            let Some(pointer) = location.strip_prefix('/') else {
                return unresolved("a JSON/YAML location is a pointer starting with /");
            };
            let tokens: Vec<String> = pointer
                .split('/')
                .map(|t| t.replace("~1", "/").replace("~0", "~"))
                .collect();
            let tree = parse(format.language().unwrap(), source)?;
            let found = if format == Format::Json {
                json_pointer(tree.root_node(), source, &tokens)
            } else {
                yaml_pointer(tree.root_node(), source, &tokens)
            };
            match found.as_slice() {
                [node] => Ok(vec![region(*node, source, 0)]),
                [] => unresolved("pointer not found"),
                _ => unresolved("pointer matches more than one value"),
            }
        }
        Format::Css => {
            let tree = parse(format.language().unwrap(), source)?;
            let wanted = normalize_css(location);
            let mut found = Vec::new();
            css_rules(tree.root_node(), source, &wanted, &mut found);
            match found.as_slice() {
                [node] => Ok(vec![region(*node, source, 0)]),
                [] => unresolved("CSS rule not found"),
                _ => unresolved("CSS rule matches more than one rule"),
            }
        }
        Format::Other => unresolved("locations are not supported for this format"),
    }
}

// ---- code symbols ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    CSharp,
    Script,
    Go,
}

fn lang_of(format: Format) -> Lang {
    match format {
        Format::CSharp => Lang::CSharp,
        Format::Go => Lang::Go,
        _ => Lang::Script,
    }
}

struct Symbol<'t> {
    path: Vec<String>,
    node: Node<'t>,
}

impl Symbol<'_> {
    /// The selector names the dotted path; a segment may itself contain dots, such as the
    /// module name in `declare module "decimal.js"`.
    fn matches(&self, name: &str) -> bool {
        self.path.join(".") == name
    }
}

fn pick_symbols(
    location: &str,
    symbols: Vec<Symbol>,
    source: &[u8],
    offset: usize,
) -> Result<Vec<Region>, Failure> {
    let (name, grouped) = match location.strip_suffix("(*)") {
        Some(name) => (name, true),
        None => (location, false),
    };
    let mut matches: Vec<&Symbol> = symbols.iter().filter(|s| s.matches(name)).collect();
    matches.sort_by_key(|s| s.node.start_byte());
    match matches.len() {
        0 => unresolved("symbol not found"),
        1 => Ok(vec![region(matches[0].node, source, offset)]),
        _ if grouped => Ok(matches
            .iter()
            .map(|s| region(s.node, source, offset))
            .collect()),
        _ => unresolved("same-named members are ambiguous; name(*) groups them"),
    }
}

fn text<'s>(node: Node, source: &'s [u8]) -> &'s str {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("")
}

fn named_children<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn children_by_field<'t>(node: Node<'t>, field: &str) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.children_by_field_name(field, &mut cursor).collect()
}

fn push<'t>(out: &mut Vec<Symbol<'t>>, prefix: &[String], name: &str, node: Node<'t>) {
    let mut path = prefix.to_vec();
    path.push(name.to_string());
    out.push(Symbol { path, node });
}

/// Declarations as dotted paths of their enclosing named declarations.
fn collect<'t>(
    node: Node<'t>,
    source: &[u8],
    lang: Lang,
    prefix: &mut Vec<String>,
    out: &mut Vec<Symbol<'t>>,
) {
    for child in named_children(node) {
        match lang {
            Lang::CSharp => collect_csharp(child, source, prefix, out),
            Lang::Script => collect_script(child, source, prefix, out),
            Lang::Go => collect_go(child, source, prefix, out),
        }
    }
}

/// Declares `node` under `name` and descends into its `body` field as a container.
fn container<'t>(
    node: Node<'t>,
    name: &str,
    source: &[u8],
    lang: Lang,
    prefix: &mut Vec<String>,
    out: &mut Vec<Symbol<'t>>,
) {
    container_path(node, &[name], source, lang, prefix, out);
}

/// `container` for a name of several segments, such as `namespace Foo.Bar`.
fn container_path<'t>(
    node: Node<'t>,
    segments: &[&str],
    source: &[u8],
    lang: Lang,
    prefix: &mut Vec<String>,
    out: &mut Vec<Symbol<'t>>,
) {
    let depth = prefix.len();
    prefix.extend(segments.iter().map(|s| s.to_string()));
    out.push(Symbol {
        path: prefix.clone(),
        node,
    });
    if let Some(body) = node.child_by_field_name("body") {
        collect(body, source, lang, prefix, out);
    }
    prefix.truncate(depth);
}

fn collect_csharp<'t>(
    node: Node<'t>,
    source: &[u8],
    prefix: &mut Vec<String>,
    out: &mut Vec<Symbol<'t>>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| text(n, source).to_string());
    match node.kind() {
        // Namespaces do not appear in selectors.
        "namespace_declaration" => {
            if let Some(body) = node.child_by_field_name("body") {
                collect(body, source, Lang::CSharp, prefix, out);
            }
        }
        "class_declaration"
        | "struct_declaration"
        | "interface_declaration"
        | "record_declaration"
        | "enum_declaration" => {
            if let Some(name) = name {
                container(node, &name, source, Lang::CSharp, prefix, out);
            }
        }
        "method_declaration"
        | "constructor_declaration"
        | "property_declaration"
        | "event_declaration"
        | "delegate_declaration"
        | "enum_member_declaration"
        | "local_function_statement" => {
            if let Some(name) = name {
                push(out, prefix, &name, node);
            }
        }
        // Declarations under `#if` and similar directives belong to the enclosing scope.
        kind if kind.starts_with("preproc_") => collect(node, source, Lang::CSharp, prefix, out),
        "field_declaration" | "event_field_declaration" => {
            for declaration in named_children(node)
                .into_iter()
                .filter(|n| n.kind() == "variable_declaration")
            {
                for declarator in named_children(declaration)
                    .into_iter()
                    .filter(|n| n.kind() == "variable_declarator")
                {
                    if let Some(name) = declarator.child_by_field_name("name") {
                        push(out, prefix, text(name, source), node);
                    }
                }
            }
        }
        _ => {}
    }
}

fn collect_script<'t>(
    node: Node<'t>,
    source: &[u8],
    prefix: &mut Vec<String>,
    out: &mut Vec<Symbol<'t>>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| text(n, source).to_string());
    match node.kind() {
        "expression_statement" => collect(node, source, Lang::Script, prefix, out),
        // The wrapper is part of the declaration's evidence: removing `export` changes the digest.
        "export_statement" | "ambient_declaration" => {
            let first = out.len();
            collect(node, source, Lang::Script, prefix, out);
            for symbol in &mut out[first..] {
                if symbol.node.parent() == Some(node) {
                    symbol.node = node;
                }
            }
        }
        "class_declaration" | "abstract_class_declaration" | "interface_declaration" => {
            if let Some(name) = name {
                container(node, &name, source, Lang::Script, prefix, out);
            }
        }
        // `namespace Foo.Bar`, `module M`, `declare module "fs"`: the name's segments nest.
        "internal_module" | "module" => {
            if let Some(name) = node.child_by_field_name("name") {
                let raw = text(name, source);
                let segments: Vec<&str> = if name.kind() == "string" {
                    vec![raw.trim_matches(['"', '\''])]
                } else {
                    raw.split('.').map(str::trim).collect()
                };
                container_path(node, &segments, source, Lang::Script, prefix, out);
            }
        }
        "enum_declaration" => {
            let (Some(name), Some(body)) = (name, node.child_by_field_name("body")) else {
                return;
            };
            push(out, prefix, &name, node);
            prefix.push(name);
            for member in named_children(body) {
                let name = match member.kind() {
                    "property_identifier" | "string" => Some(member),
                    "enum_assignment" => member.child_by_field_name("name"),
                    _ => None,
                };
                if let Some(name) = name {
                    push(
                        out,
                        prefix,
                        text(name, source).trim_matches(['"', '\'']),
                        member,
                    );
                }
            }
            prefix.pop();
        }
        "function_declaration"
        | "generator_function_declaration"
        | "function_signature"
        | "type_alias_declaration"
        | "method_definition"
        | "method_signature"
        | "abstract_method_signature"
        | "public_field_definition"
        | "property_signature" => {
            if let Some(name) = name {
                push(out, prefix, &name, node);
            }
        }
        "field_definition" => {
            if let Some(property) = node.child_by_field_name("property") {
                push(out, prefix, text(property, source), node);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let declarators: Vec<Node> = named_children(node)
                .into_iter()
                .filter(|n| n.kind() == "variable_declarator")
                .collect();
            let single = declarators.len() == 1;
            for declarator in declarators {
                if let Some(name) = declarator
                    .child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")
                {
                    push(
                        out,
                        prefix,
                        text(name, source),
                        if single { node } else { declarator },
                    );
                }
            }
        }
        _ => {}
    }
}

fn collect_go<'t>(node: Node<'t>, source: &[u8], prefix: &[String], out: &mut Vec<Symbol<'t>>) {
    match node.kind() {
        "function_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                push(out, prefix, text(name, source), node);
            }
        }
        "method_declaration" => {
            let receiver = node
                .child_by_field_name("receiver")
                .and_then(|r| {
                    named_children(r)
                        .into_iter()
                        .find(|n| n.kind() == "parameter_declaration")
                })
                .and_then(|p| p.child_by_field_name("type"))
                .and_then(|t| go_type_name(t, source));
            if let (Some(receiver), Some(name)) = (receiver, node.child_by_field_name("name")) {
                push(out, &[receiver], text(name, source), node);
            }
        }
        "type_declaration" => {
            let specs: Vec<Node> = named_children(node)
                .into_iter()
                .filter(|n| matches!(n.kind(), "type_spec" | "type_alias"))
                .collect();
            let single = specs.len() == 1;
            for spec in specs {
                let Some(name) = spec.child_by_field_name("name") else {
                    continue;
                };
                let name = text(name, source).to_string();
                push(out, prefix, &name, if single { node } else { spec });
                if let Some(ty) = spec.child_by_field_name("type") {
                    go_members(ty, source, &name, out);
                }
            }
        }
        "const_declaration" | "var_declaration" => {
            let mut specs = Vec::new();
            for child in named_children(node) {
                match child.kind() {
                    "const_spec" | "var_spec" => specs.push(child),
                    "var_spec_list" => specs.extend(
                        named_children(child)
                            .into_iter()
                            .filter(|n| n.kind() == "var_spec"),
                    ),
                    _ => {}
                }
            }
            let single = specs.len() == 1;
            for spec in specs {
                let names: Vec<Node> = children_by_field(spec, "name")
                    .into_iter()
                    .filter(|n| n.kind() == "identifier")
                    .collect();
                let whole = single && names.len() == 1;
                for name in names {
                    push(
                        out,
                        prefix,
                        text(name, source),
                        if whole { node } else { spec },
                    );
                }
            }
        }
        _ => {}
    }
}

fn go_type_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(text(node, source).to_string()),
        "pointer_type" => named_children(node)
            .into_iter()
            .find_map(|n| go_type_name(n, source)),
        "generic_type" => node
            .child_by_field_name("type")
            .and_then(|n| go_type_name(n, source)),
        _ => None,
    }
}

fn go_members<'t>(ty: Node<'t>, source: &[u8], owner: &str, out: &mut Vec<Symbol<'t>>) {
    let prefix = [owner.to_string()];
    match ty.kind() {
        "struct_type" => {
            for list in named_children(ty)
                .into_iter()
                .filter(|n| n.kind() == "field_declaration_list")
            {
                for field in named_children(list)
                    .into_iter()
                    .filter(|n| n.kind() == "field_declaration")
                {
                    for name in children_by_field(field, "name") {
                        push(out, &prefix, text(name, source), field);
                    }
                }
            }
        }
        "interface_type" => {
            for method in named_children(ty)
                .into_iter()
                .filter(|n| n.kind() == "method_elem")
            {
                if let Some(name) = method.child_by_field_name("name") {
                    push(out, &prefix, text(name, source), method);
                }
            }
        }
        _ => {}
    }
}

/// Symbols of the `<script>` blocks of a Vue single-file component.
fn vue_regions(source: &[u8], location: &str) -> Result<Vec<Region>, Failure> {
    let blocks = vue_scripts(source);
    if blocks.is_empty() {
        return unresolved("no <script> block");
    }
    let trees: Vec<(Tree, usize, Lang)> = blocks
        .iter()
        .map(|(range, format)| {
            Ok((
                parse(format.language().unwrap(), &source[range.clone()])?,
                range.start,
                lang_of(*format),
            ))
        })
        .collect::<Result<_, Failure>>()?;
    let mut regions = Vec::new();
    let mut found = Vec::new();
    for (tree, start, lang) in &trees {
        let block = &source[*start..];
        let mut symbols = Vec::new();
        collect(
            tree.root_node(),
            block,
            *lang,
            &mut Vec::new(),
            &mut symbols,
        );
        found.extend(symbols.into_iter().map(|s| (s, *start)));
    }
    // Pick across blocks as one symbol table.
    let (name, grouped) = match location.strip_suffix("(*)") {
        Some(name) => (name, true),
        None => (location, false),
    };
    let mut matches: Vec<&(Symbol, usize)> =
        found.iter().filter(|(s, _)| s.matches(name)).collect();
    matches.sort_by_key(|(s, start)| start + s.node.start_byte());
    if matches.len() > 1 && !grouped {
        return unresolved("same-named members are ambiguous; name(*) groups them");
    }
    for (symbol, start) in matches {
        regions.push(region(symbol.node, &source[*start..], *start));
    }
    if regions.is_empty() {
        return unresolved("symbol not found");
    }
    Ok(regions)
}

/// Content ranges of top-level `<script>` blocks with their script format. Only top-level
/// elements count; HTML comments and nested markup are skipped.
fn vue_scripts(source: &[u8]) -> Vec<(Range<usize>, Format)> {
    let Ok(text) = std::str::from_utf8(source) else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    let mut at = 0;
    while let Some(lt) = text[at..].find('<') {
        let start = at + lt;
        if text[start..].starts_with("<!--") {
            match text[start + 4..].find("-->") {
                Some(end) => at = start + 4 + end + 3,
                None => break,
            }
            continue;
        }
        let Some(tag) = start_tag(text, start) else {
            at = start + 1;
            continue;
        };
        if tag.self_closing {
            at = tag.end;
            continue;
        }
        let raw = matches!(tag.name, "script" | "style");
        let Some((close, after)) = closing_tag(text, tag.end, tag.name, raw) else {
            break;
        };
        if tag.name == "script" {
            let format = match tag.lang.as_deref() {
                Some("ts") => Format::TypeScript,
                Some("tsx") => Format::Tsx,
                _ => Format::JavaScript,
            };
            blocks.push((tag.end..close, format));
        }
        at = after;
    }
    blocks
}

struct StartTag<'s> {
    name: &'s str,
    lang: Option<String>,
    self_closing: bool,
    /// Offset just past `>`.
    end: usize,
}

fn start_tag(text: &str, start: usize) -> Option<StartTag<'_>> {
    let rest = &text[start + 1..];
    let name_len = rest
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(rest.len());
    let name = &rest[..name_len];
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut chars = rest[name_len..].char_indices().peekable();
    let mut lang = None;
    let mut self_closing = false;
    loop {
        let (i, c) = chars.next()?;
        match c {
            '>' => {
                return Some(StartTag {
                    name,
                    lang,
                    self_closing,
                    end: start + 1 + name_len + i + 1,
                });
            }
            '/' => self_closing = true,
            c if c.is_whitespace() => {}
            _ => {
                self_closing = false;
                let mut key = c.to_string();
                while let Some(&(_, k)) = chars.peek() {
                    if k.is_whitespace() || matches!(k, '=' | '>' | '/') {
                        break;
                    }
                    key.push(k);
                    chars.next();
                }
                while chars.peek().is_some_and(|&(_, k)| k.is_whitespace()) {
                    chars.next();
                }
                if chars.peek().is_some_and(|&(_, k)| k == '=') {
                    chars.next();
                    while chars.peek().is_some_and(|&(_, k)| k.is_whitespace()) {
                        chars.next();
                    }
                    let mut value = String::new();
                    match chars.peek().map(|&(_, k)| k) {
                        Some(q @ ('"' | '\'')) => {
                            chars.next();
                            for (_, k) in chars.by_ref() {
                                if k == q {
                                    break;
                                }
                                value.push(k);
                            }
                        }
                        _ => {
                            while let Some(&(_, k)) = chars.peek() {
                                if k.is_whitespace() || k == '>' {
                                    break;
                                }
                                value.push(k);
                                chars.next();
                            }
                        }
                    }
                    if key.eq_ignore_ascii_case("lang") {
                        lang = Some(value);
                    }
                }
            }
        }
    }
}

/// Offset of the `</name` closing an element opened before `from`, and the offset past its `>`.
/// Raw-text elements end at the first `</name`; others count nested same-named elements.
fn closing_tag(text: &str, from: usize, name: &str, raw: bool) -> Option<(usize, usize)> {
    let close = format!("</{name}");
    let mut depth = 0;
    let mut at = from;
    loop {
        let lt = at + text[at..].find('<')?;
        let rest = &text[lt..];
        if !raw && rest.starts_with("<!--") {
            at = lt + 4 + text[lt + 4..].find("-->")? + 3;
        } else if rest.starts_with(&close) && is_name_end(&rest[close.len()..]) {
            let after = lt + rest.find('>')? + 1;
            if depth == 0 {
                return Some((lt, after));
            }
            depth -= 1;
            at = after;
        } else if !raw && rest[1..].starts_with(name) && is_name_end(&rest[1 + name.len()..]) {
            let tag = start_tag(text, lt)?;
            if !tag.self_closing {
                depth += 1;
            }
            at = tag.end;
        } else {
            at = lt + 1;
        }
    }
}

fn is_name_end(rest: &str) -> bool {
    rest.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/')
}

// ---- JSON / YAML pointers ----

fn json_pointer<'t>(root: Node<'t>, source: &[u8], tokens: &[String]) -> Vec<Node<'t>> {
    let values: Vec<Node> = named_children(root)
        .into_iter()
        .filter(|n| n.kind() != "comment")
        .collect();
    let mut current = values;
    for token in tokens {
        let mut next = Vec::new();
        for node in current {
            match node.kind() {
                "object" => {
                    for pair in named_children(node)
                        .into_iter()
                        .filter(|n| n.kind() == "pair")
                    {
                        let key = pair.child_by_field_name("key").map(|k| text(k, source));
                        let key: Option<String> = key.and_then(|k| serde_json::from_str(k).ok());
                        if key.as_deref() == Some(token.as_str()) {
                            next.extend(pair.child_by_field_name("value"));
                        }
                    }
                }
                "array" => {
                    let items: Vec<Node> = named_children(node)
                        .into_iter()
                        .filter(|n| n.kind() != "comment")
                        .collect();
                    if let Some(item) = array_index(token).and_then(|i| items.get(i)) {
                        next.push(*item);
                    }
                }
                _ => {}
            }
        }
        current = next;
    }
    current
}

fn array_index(token: &str) -> Option<usize> {
    if token == "0" || (!token.starts_with('0') && token.bytes().all(|b| b.is_ascii_digit())) {
        token.parse().ok()
    } else {
        None
    }
}

/// The collection or scalar inside a YAML node, past anchors and tags.
fn yaml_content(node: Node) -> Option<Node> {
    match node.kind() {
        "block_node" | "flow_node" => named_children(node)
            .into_iter()
            .find(|n| !matches!(n.kind(), "anchor" | "tag" | "comment")),
        _ => Some(node),
    }
}

fn yaml_pointer<'t>(root: Node<'t>, source: &[u8], tokens: &[String]) -> Vec<Node<'t>> {
    let mut current: Vec<Node> = Vec::new();
    for document in named_children(root)
        .into_iter()
        .filter(|n| n.kind() == "document")
    {
        current.extend(
            named_children(document)
                .into_iter()
                .filter(|n| matches!(n.kind(), "block_node" | "flow_node")),
        );
    }
    for token in tokens {
        let mut next = Vec::new();
        for node in current {
            let Some(content) = yaml_content(node) else {
                continue;
            };
            match content.kind() {
                "block_mapping" | "flow_mapping" => {
                    for pair in named_children(content)
                        .into_iter()
                        .filter(|n| matches!(n.kind(), "block_mapping_pair" | "flow_pair"))
                    {
                        let key = pair
                            .child_by_field_name("key")
                            .and_then(|k| yaml_scalar(k, source));
                        if key.as_deref() == Some(token.as_str()) {
                            next.push(pair.child_by_field_name("value").unwrap_or(pair));
                        }
                    }
                }
                "block_sequence" => {
                    let items: Vec<Node> = named_children(content)
                        .into_iter()
                        .filter(|n| n.kind() == "block_sequence_item")
                        .collect();
                    if let Some(item) = array_index(token).and_then(|i| items.get(i)) {
                        next.extend(
                            named_children(*item)
                                .into_iter()
                                .find(|n| n.kind() != "comment"),
                        );
                    }
                }
                "flow_sequence" => {
                    let items: Vec<Node> = named_children(content)
                        .into_iter()
                        .filter(|n| n.kind() == "flow_node")
                        .collect();
                    if let Some(item) = array_index(token).and_then(|i| items.get(i)) {
                        next.push(*item);
                    }
                }
                _ => {}
            }
        }
        current = next;
    }
    current
}

fn yaml_scalar(node: Node, source: &[u8]) -> Option<String> {
    let scalar = yaml_content(node)?;
    let raw = text(scalar, source);
    match scalar.kind() {
        "plain_scalar" => Some(raw.trim().to_string()),
        "single_quote_scalar" => Some(raw.get(1..raw.len().saturating_sub(1))?.replace("''", "'")),
        "double_quote_scalar" => serde_json::from_str(raw)
            .ok()
            .or_else(|| Some(raw.get(1..raw.len().saturating_sub(1))?.to_string())),
        _ => None,
    }
}

// ---- CSS rules ----

/// Selector text with whitespace collapsed and dropped around combinators and commas,
/// outside strings and escapes, which are exact data.
fn normalize_css(selector: &str) -> String {
    let mut out = String::new();
    let mut chars = selector.trim().chars();
    let mut space = false;
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        let combinator = matches!(c, ',' | '>' | '+' | '~');
        if space && !combinator && !out.ends_with([',', '>', '+', '~']) {
            out.push(' ');
        }
        space = false;
        out.push(c);
        match c {
            '\\' => out.extend(chars.next()),
            '"' | '\'' => {
                while let Some(d) = chars.next() {
                    out.push(d);
                    if d == '\\' {
                        out.extend(chars.next());
                    } else if d == c {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn css_rules<'t>(node: Node<'t>, source: &[u8], wanted: &str, out: &mut Vec<Node<'t>>) {
    for child in named_children(node) {
        if child.kind() == "rule_set" {
            let selectors = named_children(child)
                .into_iter()
                .find(|n| n.kind() == "selectors");
            if selectors.is_some_and(|s| normalize_css(text(s, source)) == wanted) {
                out.push(child);
            }
        }
        css_rules(child, source, wanted, out);
    }
}
