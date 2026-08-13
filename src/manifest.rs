use rapidhash::fast::{RapidHashMap as HashMap, RapidHashSet as HashSet};
use rapidhash::{HashMapExt, HashSetExt};
use std::borrow::Cow;
use std::fmt;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SUPPORTED_SYNTAX_VERSION: &str = "1.14.0";

#[cfg(windows)]
type EdgeSlashKey = (usize, usize);
#[cfg(windows)]
type EdgeSlashBits = HashMap<EdgeSlashKey, Box<[u64]>>;

#[derive(Clone, Debug, Default)]
pub struct Rule {
    pub name: String,
    pub bindings: HashMap<String, String>,
    pub source: Arc<PathBuf>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Pool {
    pub name: String,
    pub depth: usize,
    pub depth_specified: bool,
    pub source: Arc<PathBuf>,
    pub line: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Edge {
    pub explicit_outputs: Vec<String>,
    pub implicit_outputs: Vec<String>,
    pub rule: String,
    pub explicit_inputs: Vec<String>,
    pub implicit_inputs: Vec<String>,
    pub order_only_inputs: Vec<String>,
    pub validations: Vec<String>,
    pub bindings: HashMap<String, String>,
    pub source: Arc<PathBuf>,
    pub line: usize,
    pub scope: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Scope {
    parent: Option<usize>,
    variables: HashMap<String, String>,
    rules: HashMap<String, Rule>,
}

impl Edge {
    pub fn outputs(&self) -> impl Iterator<Item = &str> {
        self.explicit_outputs
            .iter()
            .chain(&self.implicit_outputs)
            .map(String::as_str)
    }

    pub fn inputs(&self) -> impl Iterator<Item = &str> {
        self.explicit_inputs
            .iter()
            .chain(&self.implicit_inputs)
            .chain(&self.order_only_inputs)
            .map(String::as_str)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub variables: HashMap<String, String>,
    pub rules: HashMap<String, Rule>,
    pub pools: HashMap<String, Pool>,
    pub edges: Vec<Edge>,
    pub defaults: Vec<String>,
    pub phony_self_references: Vec<String>,
    pub warnings: Vec<String>,
    pub root: PathBuf,
    scopes: Vec<Scope>,
    cyclic_rules: HashMap<usize, HashSet<String>>,
    has_pool_binding: bool,
    has_dyndep_binding: bool,
    has_dependency_binding: bool,
    #[cfg(windows)]
    path_slash_bits: Option<HashMap<String, u64>>,
    #[cfg(windows)]
    edge_slash_bits: Option<EdgeSlashBits>,
}

impl Manifest {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            scopes: vec![Scope::default()],
            ..Self::default()
        }
    }

    pub fn lookup_variable(&self, mut scope: usize, name: &str) -> Option<&str> {
        loop {
            let current = self.scopes.get(scope)?;
            if let Some(value) = current.variables.get(name) {
                return Some(value);
            }
            scope = current.parent?;
        }
    }

    pub fn lookup_rule(&self, mut scope: usize, name: &str) -> Option<&Rule> {
        loop {
            let current = self.scopes.get(scope)?;
            if let Some(rule) = current.rules.get(name) {
                return Some(rule);
            }
            scope = current.parent?;
        }
    }

    fn lookup_rule_scope(&self, mut scope: usize, name: &str) -> Option<usize> {
        loop {
            let current = self.scopes.get(scope)?;
            if current.rules.contains_key(name) {
                return Some(scope);
            }
            scope = current.parent?;
        }
    }

    pub fn all_rules(&self) -> impl Iterator<Item = &Rule> {
        self.scopes.iter().flat_map(|scope| scope.rules.values())
    }

    pub fn has_dependency_bindings(&self) -> bool {
        self.has_dependency_binding
    }

    pub fn has_pool_bindings(&self) -> bool {
        self.has_pool_binding
    }

    pub(crate) fn explicit_output_slash_bits(&self, edge: &Edge) -> &[u64] {
        #[cfg(windows)]
        {
            self.edge_slash_bits
                .as_ref()
                .and_then(|edges| edges.get(&edge_slash_key(edge)))
                .and_then(|bits| bits.get(..edge.explicit_outputs.len()))
                .unwrap_or(&[])
        }
        #[cfg(not(windows))]
        {
            let _ = edge;
            &[]
        }
    }

    pub(crate) fn explicit_input_slash_bits(&self, edge: &Edge) -> &[u64] {
        #[cfg(windows)]
        {
            self.edge_slash_bits
                .as_ref()
                .and_then(|edges| edges.get(&edge_slash_key(edge)))
                .and_then(|bits| bits.get(edge.explicit_outputs.len()..))
                .unwrap_or(&[])
        }
        #[cfg(not(windows))]
        {
            let _ = edge;
            &[]
        }
    }

    fn sync_root_scope(&mut self) {
        self.variables = self.scopes[0].variables.clone();
        self.rules = self.scopes[0].rules.clone();
    }
}

#[cfg(windows)]
fn edge_slash_key(edge: &Edge) -> EdgeSlashKey {
    (Arc::as_ptr(&edge.source) as usize, edge.line)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub source_line: Option<String>,
}

impl Diagnostic {
    fn new(path: &Path, line: usize, column: usize, message: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            line,
            column,
            message: message.into(),
            source_line: None,
        }
    }

    fn with_source(mut self, source: &str) -> Self {
        self.source_line = Some(source.to_owned());
        self
    }

    pub fn ninja_message(&self) -> String {
        let mut message = format!("{}:{}: {}", self.path.display(), self.line, self.message);
        if let Some(line) = &self.source_line {
            let disk_source = fs::read_to_string(&self.path).ok();
            let line = disk_source
                .as_deref()
                .and_then(|source| {
                    source
                        .split_terminator('\n')
                        .nth(self.line.saturating_sub(1))
                })
                .unwrap_or(line);
            message.push('\n');
            message.push_str(line);
            if cfg!(windows) && line.ends_with('\r') {
                // Ninja's Windows CRT translates the newline it emits after
                // a source line even when that line retained its CR byte.
                message.push('\r');
            }
            message.push('\n');
            message.extend(std::iter::repeat_n(' ', self.column.saturating_sub(1)));
            message.push_str("^ near here");
        } else {
            // Ninja's lexer diagnostics without source context retain their
            // terminating newline before the CLI adds its own.
            message.push('\n');
        }
        message
    }

    pub fn ninja_manifest_load_message(&self) -> Option<String> {
        let cause = self.message.strip_prefix("loading manifest: ")?;
        let cause = io_error_message(cause);
        let mut message = format!("loading '{}': {cause}", self.path.display());
        if cfg!(windows) {
            message.push_str("\r\r\n");
        }
        Some(message)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{}:{}:{}: error: {}",
            self.path.display(),
            self.line,
            self.column,
            self.message
        )?;
        if let Some(line) = &self.source_line {
            writeln!(f, "  {line}")?;
            write!(
                f,
                "  {:>width$}^",
                "",
                width = self.column.saturating_sub(1)
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostic {}

#[derive(Clone, Copy, Debug)]
enum Parent {
    Rule,
    Edge,
    Pool,
}

#[derive(Clone, Copy)]
enum DeferredPathKind {
    ExplicitOutput,
    ImplicitOutput,
    ExplicitInput,
    ImplicitInput,
    OrderOnlyInput,
    Validation,
}

#[derive(Clone, Copy)]
struct ParsedPath {
    kind: DeferredPathKind,
    index: usize,
    deferred: bool,
    #[cfg(windows)]
    slash_bits: u64,
}

#[derive(Default)]
struct ParsedEdgePaths(Vec<ParsedPath>);

impl ParsedEdgePaths {
    #[inline]
    fn push(&mut self, kind: DeferredPathKind, index: usize, is_deferred: bool, slash_bits: u64) {
        #[cfg(not(windows))]
        let _ = slash_bits;
        self.0.push(ParsedPath {
            kind,
            index,
            deferred: is_deferred,
            #[cfg(windows)]
            slash_bits,
        });
    }
}

fn expand_deferred_paths(
    manifest: &mut Manifest,
    edge_id: usize,
    paths: &mut ParsedEdgePaths,
) -> Result<(), Diagnostic> {
    if paths.0.is_empty() {
        #[cfg(windows)]
        if manifest.path_slash_bits.is_none() {
            return Ok(());
        }
        #[cfg(not(windows))]
        return Ok(());
    }
    let mut edge = std::mem::take(&mut manifest.edges[edge_id]);
    let result = (|| {
        for parsed in &mut paths.0 {
            if !parsed.deferred {
                continue;
            }
            let kind = parsed.kind;
            let index = parsed.index;
            let raw = match kind {
                DeferredPathKind::ExplicitOutput => {
                    std::mem::take(&mut edge.explicit_outputs[index])
                }
                DeferredPathKind::ImplicitOutput => {
                    std::mem::take(&mut edge.implicit_outputs[index])
                }
                DeferredPathKind::ExplicitInput => std::mem::take(&mut edge.explicit_inputs[index]),
                DeferredPathKind::ImplicitInput => std::mem::take(&mut edge.implicit_inputs[index]),
                DeferredPathKind::OrderOnlyInput => {
                    std::mem::take(&mut edge.order_only_inputs[index])
                }
                DeferredPathKind::Validation => std::mem::take(&mut edge.validations[index]),
            };
            let expanded = expand(&raw, |name| {
                edge.bindings.get(name).cloned().or_else(|| {
                    manifest
                        .lookup_variable(edge.scope, name)
                        .map(str::to_owned)
                })
            });
            if expanded.is_empty() {
                return Err(Diagnostic::new(
                    edge.source.as_path(),
                    edge.line + edge.bindings.len() + 1,
                    1,
                    "empty path",
                ));
            }
            let (expanded, slash_bits) = canonicalize_owned_path_with_bits(expanded);
            #[cfg(not(windows))]
            let _ = slash_bits;
            #[cfg(windows)]
            {
                parsed.slash_bits = slash_bits;
            }
            match kind {
                DeferredPathKind::ExplicitOutput => edge.explicit_outputs[index] = expanded,
                DeferredPathKind::ImplicitOutput => edge.implicit_outputs[index] = expanded,
                DeferredPathKind::ExplicitInput => edge.explicit_inputs[index] = expanded,
                DeferredPathKind::ImplicitInput => edge.implicit_inputs[index] = expanded,
                DeferredPathKind::OrderOnlyInput => edge.order_only_inputs[index] = expanded,
                DeferredPathKind::Validation => edge.validations[index] = expanded,
            }
        }
        #[cfg(windows)]
        register_edge_slash_bits(manifest, edge_id, &mut edge, paths);
        Ok(())
    })();
    manifest.edges[edge_id] = edge;
    result
}

#[cfg(windows)]
fn register_edge_slash_bits(
    manifest: &mut Manifest,
    edge_id: usize,
    edge: &mut Edge,
    paths: &ParsedEdgePaths,
) {
    if manifest.path_slash_bits.is_none() {
        if !paths.0.iter().any(|path| path.slash_bits != 0) {
            return;
        }
        let mut registry = HashMap::with_capacity(edge_id.saturating_mul(2));
        for previous in &manifest.edges[..edge_id] {
            for path in previous
                .outputs()
                .chain(previous.inputs())
                .chain(previous.validations.iter().map(String::as_str))
            {
                registry.entry(path.to_owned()).or_insert(0);
            }
        }
        manifest.path_slash_bits = Some(registry);
    }

    let path_count = edge.outputs().count() + edge.inputs().count() + edge.validations.len();
    let mut parsed_bits = vec![0u64; path_count];
    for parsed in &paths.0 {
        parsed_bits[edge_path_index(edge, parsed.kind, parsed.index)] = parsed.slash_bits;
    }
    let registry = manifest.path_slash_bits.as_mut().unwrap();
    let mut explicit_bits =
        Vec::with_capacity(edge.explicit_outputs.len() + edge.explicit_inputs.len());
    let mut bit_index = 0usize;
    let mut register = |path: &str| {
        let parsed = parsed_bits[bit_index];
        bit_index += 1;
        *registry.entry(path.to_owned()).or_insert(parsed)
    };
    for path in &edge.explicit_outputs {
        explicit_bits.push(register(path));
    }
    for path in &edge.implicit_outputs {
        register(path);
    }
    for path in &edge.explicit_inputs {
        explicit_bits.push(register(path));
    }
    for path in &edge.implicit_inputs {
        register(path);
    }
    for path in &edge.order_only_inputs {
        register(path);
    }
    for path in &edge.validations {
        register(path);
    }
    debug_assert_eq!(bit_index, path_count);
    if explicit_bits.iter().any(|bits| *bits != 0) {
        manifest
            .edge_slash_bits
            .get_or_insert_with(HashMap::new)
            .insert(edge_slash_key(edge), explicit_bits.into_boxed_slice());
    }
}

#[cfg(windows)]
fn edge_path_index(edge: &Edge, kind: DeferredPathKind, index: usize) -> usize {
    let outputs = edge.explicit_outputs.len() + edge.implicit_outputs.len();
    let explicit_inputs = outputs + edge.explicit_inputs.len();
    let implicit_inputs = explicit_inputs + edge.implicit_inputs.len();
    let order_only_inputs = implicit_inputs + edge.order_only_inputs.len();
    match kind {
        DeferredPathKind::ExplicitOutput => index,
        DeferredPathKind::ImplicitOutput => edge.explicit_outputs.len() + index,
        DeferredPathKind::ExplicitInput => outputs + index,
        DeferredPathKind::ImplicitInput => explicit_inputs + index,
        DeferredPathKind::OrderOnlyInput => implicit_inputs + index,
        DeferredPathKind::Validation => order_only_inputs + index,
    }
}

fn finalize_parent(
    manifest: &mut Manifest,
    scope: usize,
    parent: Option<Parent>,
    current_rule: Option<&str>,
    current_edge: Option<usize>,
    current_pool: Option<&str>,
    deferred_paths: Option<&mut ParsedEdgePaths>,
) -> Result<(), Diagnostic> {
    match parent {
        Some(Parent::Rule) => {
            let rule_name = current_rule.unwrap();
            let rule = manifest.scopes[scope].rules.get(rule_name).unwrap();
            let has_command = rule
                .bindings
                .get("command")
                .is_some_and(|command| !command.is_empty());
            if !has_command {
                return Err(Diagnostic::new(
                    rule.source.as_path(),
                    rule.line + rule.bindings.len() + 1,
                    1,
                    "expected 'command =' line",
                ));
            }
            let has_rspfile = rule
                .bindings
                .get("rspfile")
                .is_some_and(|value| !value.is_empty());
            let has_rspfile_content = rule
                .bindings
                .get("rspfile_content")
                .is_some_and(|value| !value.is_empty());
            if has_rspfile != has_rspfile_content {
                return Err(Diagnostic::new(
                    rule.source.as_path(),
                    rule.line + rule.bindings.len() + 1,
                    1,
                    "rspfile and rspfile_content need to be both specified",
                ));
            }
            let edge = Edge {
                rule: rule_name.to_owned(),
                scope,
                ..Edge::default()
            };
            if binding_cycle(manifest, &edge).is_some() {
                manifest
                    .cyclic_rules
                    .entry(scope)
                    .or_default()
                    .insert(rule_name.to_owned());
            }
        }
        Some(Parent::Pool) => {
            let pool = manifest.pools.get(current_pool.unwrap()).unwrap();
            if !pool.depth_specified {
                return Err(Diagnostic::new(
                    pool.source.as_path(),
                    pool.line + 1,
                    1,
                    "expected 'depth =' line",
                ));
            }
        }
        Some(Parent::Edge) => {
            expand_deferred_paths(manifest, current_edge.unwrap(), deferred_paths.unwrap())?;
            let edge = &manifest.edges[current_edge.unwrap()];
            if !manifest.cyclic_rules.is_empty() {
                let rule_scope = manifest
                    .lookup_rule_scope(edge.scope, &edge.rule)
                    .unwrap_or(edge.scope);
                let rule_is_cyclic = manifest
                    .cyclic_rules
                    .get(&rule_scope)
                    .is_some_and(|rules| rules.contains(edge.rule.as_str()));
                if rule_is_cyclic {
                    if let Some(cycle) = binding_cycle(manifest, edge) {
                        return Err(Diagnostic::new(
                            edge.source.as_path(),
                            edge.line,
                            1,
                            format!("cycle in rule variables: {}", cycle.join(" -> ")),
                        ));
                    }
                }
            }
            if manifest.has_pool_binding {
                let pool = evaluate_edge_binding(manifest, edge, "pool", 0);
                if !pool.is_empty() && pool != "console" && !manifest.pools.contains_key(&pool) {
                    return Err(Diagnostic::new(
                        edge.source.as_path(),
                        edge.line + edge.bindings.len() + 1,
                        1,
                        format!("unknown pool name '{pool}'"),
                    ));
                }
            }
            if manifest.has_dyndep_binding {
                let dyndep = evaluate_edge_binding(manifest, edge, "dyndep", 0);
                if !dyndep.is_empty() && !edge.inputs().any(|input| input == dyndep) {
                    return Err(Diagnostic::new(
                        edge.source.as_path(),
                        edge.line + edge.bindings.len() + 1,
                        1,
                        format!("dyndep '{dyndep}' is not an input"),
                    ));
                }
            }
        }
        None => {}
    }
    Ok(())
}

pub fn load_manifest(path: impl AsRef<Path>) -> Result<Manifest, Diagnostic> {
    let path = path.as_ref();
    let mut manifest = Manifest::new(path.to_owned());
    let mut stack = HashSet::new();
    parse_file_into(path, &mut manifest, &mut stack, 0)?;
    manifest.sync_root_scope();
    validate(&manifest)?;
    #[cfg(windows)]
    {
        manifest.path_slash_bits = None;
    }
    Ok(manifest)
}

#[derive(Clone, Hash, PartialEq, Eq)]
enum FileIdentity {
    #[cfg(windows)]
    Windows(u32, u64),
    #[cfg(unix)]
    Unix(u64, u64),
    Path(PathBuf),
}

fn file_identity(file: &fs::File, path: &Path) -> FileIdentity {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } != 0 {
            let index = (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow);
            return FileIdentity::Windows(information.dwVolumeSerialNumber, index);
        }
    }
    #[cfg(unix)]
    {
        if let Ok(metadata) = file.metadata() {
            use std::os::unix::fs::MetadataExt as _;
            return FileIdentity::Unix(metadata.dev(), metadata.ino());
        }
    }
    FileIdentity::Path(path.canonicalize().unwrap_or_else(|_| path.to_owned()))
}

pub fn parse_manifest(source: &str, path: impl AsRef<Path>) -> Result<Manifest, Diagnostic> {
    let path = path.as_ref();
    let mut manifest = Manifest::new(path.to_owned());
    parse_source_into(source, path, &mut manifest, 0)?;
    manifest.sync_root_scope();
    validate(&manifest)?;
    #[cfg(windows)]
    {
        manifest.path_slash_bits = None;
    }
    Ok(manifest)
}

fn parse_file_into(
    path: &Path,
    manifest: &mut Manifest,
    stack: &mut HashSet<FileIdentity>,
    scope: usize,
) -> Result<(), Diagnostic> {
    let mut file = fs::File::open(path)
        .map_err(|error| Diagnostic::new(path, 1, 1, format!("loading manifest: {error}")))?;
    let identity = file_identity(&file, path);
    if !stack.insert(identity.clone()) {
        return Err(Diagnostic::new(path, 1, 1, "include cycle detected"));
    }
    let mut source = String::new();
    file.read_to_string(&mut source)
        .map_err(|error| Diagnostic::new(path, 1, 1, format!("loading manifest: {error}")))?;
    parse_source_into_with_loader(&source, path, manifest, scope, Some(stack))?;
    stack.remove(&identity);
    Ok(())
}

fn parse_source_into(
    source: &str,
    path: &Path,
    manifest: &mut Manifest,
    scope: usize,
) -> Result<(), Diagnostic> {
    parse_source_into_with_loader(source, path, manifest, scope, None)
}

fn parse_source_into_with_loader(
    source: &str,
    path: &Path,
    manifest: &mut Manifest,
    scope: usize,
    mut stack: Option<&mut HashSet<FileIdentity>>,
) -> Result<(), Diagnostic> {
    let missing_final_newline = !source.is_empty() && !source.ends_with('\n');
    let source_path = Arc::new(path.to_owned());
    let logical_lines = logical_lines(source);
    let mut parent: Option<Parent> = None;
    let mut current_rule: Option<String> = None;
    let mut current_edge: Option<usize> = None;
    let mut current_pool: Option<String> = None;
    let mut deferred_paths: Option<ParsedEdgePaths> = None;
    let ninja_compat = crate::program_name() == "ninja";

    for (line_no, raw_line) in logical_lines {
        let without_comment = strip_comment(&raw_line);
        if without_comment.trim().is_empty() {
            if !raw_line.trim_start().starts_with('#') {
                finalize_parent(
                    manifest,
                    scope,
                    parent,
                    current_rule.as_deref(),
                    current_edge,
                    current_pool.as_deref(),
                    deferred_paths.as_mut(),
                )?;
                parent = None;
                current_rule = None;
                current_edge = None;
                current_pool = None;
                deferred_paths = None;
            }
            continue;
        }
        if !ninja_compat
            && !raw_line.trim_start().starts_with('#')
            && raw_line
                .chars()
                .take_while(|character| matches!(character, ' ' | '\t'))
                .any(|character| character == '\t')
        {
            let column = raw_line.find('\t').unwrap_or(0) + 1;
            return Err(
                Diagnostic::new(path, line_no, column, "tabs are not allowed, use spaces")
                    .with_source(&raw_line),
            );
        }
        let indented = if ninja_compat {
            without_comment.starts_with(' ')
        } else {
            without_comment.starts_with([' ', '\t'])
        };
        let line = without_comment.trim_end();
        if source.ends_with('\n')
            && raw_line
                .as_bytes()
                .iter()
                .rev()
                .take_while(|byte| **byte == b'$')
                .count()
                % 2
                == 1
        {
            let eof_line = source.bytes().filter(|byte| *byte == b'\n').count() + 1;
            return Err(Diagnostic::new(path, eof_line, 1, "unexpected EOF"));
        }
        let (invalid_escape, newline_escape) = dollar_escape_issues(line);
        let newline_allowed = newline_escape.is_none()
            || manifest
                .lookup_variable(scope, "ninja_required_version")
                .is_some_and(version_supports_newline_escape);
        let disallowed_newline = newline_escape.filter(|_| !newline_allowed);
        let issue = match (invalid_escape, disallowed_newline) {
            (Some(invalid), Some(newline)) if newline < invalid => Some((newline, true)),
            (Some(invalid), _) => Some((invalid, false)),
            (None, Some(newline)) => Some((newline, true)),
            (None, None) => None,
        };
        if let Some((column, newline)) = issue {
            return Err(diagnostic_for_logical_line(
                path,
                line_no,
                column + 1,
                if newline {
                    "using $^ escape requires specifying 'ninja_required_version' with version greater or equal 1.14"
                } else {
                    "bad $-escape (literal $ must be written as $$)"
                },
                &raw_line,
                source,
            ));
        }

        if indented {
            let binding = line.trim_start();
            let (key, value) = parse_binding(binding).ok_or_else(|| {
                let column = raw_line.len() - raw_line.trim_start().len() + 1;
                Diagnostic::new(path, line_no, column, "expected variable name")
                    .with_source(&raw_line)
            })?;
            manifest.has_pool_binding |= key == "pool";
            manifest.has_dyndep_binding |= key == "dyndep";
            manifest.has_dependency_binding |= matches!(key, "deps" | "depfile");
            match parent {
                Some(Parent::Rule) => {
                    if !matches!(
                        key,
                        "command"
                            | "depfile"
                            | "deps"
                            | "description"
                            | "dyndep"
                            | "generator"
                            | "msvc_deps_prefix"
                            | "pool"
                            | "restat"
                            | "rspfile"
                            | "rspfile_content"
                    ) {
                        return Err(Diagnostic::new(
                            path,
                            line_no,
                            line_end_column(&raw_line),
                            format!("unexpected variable '{key}'"),
                        )
                        .with_source(&raw_line));
                    }
                    let rule = manifest.scopes[scope]
                        .rules
                        .get_mut(current_rule.as_ref().unwrap())
                        .unwrap();
                    rule.bindings.insert(key.to_owned(), value.to_owned());
                }
                Some(Parent::Edge) => {
                    let expanded = expand(value, |name| {
                        manifest.lookup_variable(scope, name).map(str::to_owned)
                    });
                    manifest.edges[current_edge.unwrap()]
                        .bindings
                        .insert(key.to_owned(), expanded);
                }
                Some(Parent::Pool) => {
                    if key != "depth" {
                        let message = if ninja_compat {
                            format!("unexpected variable '{key}'")
                        } else {
                            format!("unexpected pool variable '{key}'")
                        };
                        return Err(Diagnostic::new(
                            path,
                            line_no,
                            line_end_column(&raw_line),
                            message,
                        )
                        .with_source(&raw_line));
                    }
                    let expanded = expand(value, |name| {
                        manifest.lookup_variable(scope, name).map(str::to_owned)
                    });
                    let depth = expanded.parse::<usize>().map_err(|_| {
                        Diagnostic::new(
                            path,
                            line_no,
                            line_end_column(&raw_line),
                            "invalid pool depth",
                        )
                        .with_source(&raw_line)
                    })?;
                    let pool = manifest
                        .pools
                        .get_mut(current_pool.as_ref().unwrap())
                        .unwrap();
                    pool.depth = depth;
                    pool.depth_specified = true;
                }
                None => {
                    if ninja_compat {
                        return Err(Diagnostic::new(path, line_no, 1, "unexpected indent"));
                    }
                    return Err(Diagnostic::new(path, line_no, 1, "unexpected indentation")
                        .with_source(&raw_line));
                }
            }
            continue;
        }

        finalize_parent(
            manifest,
            scope,
            parent,
            current_rule.as_deref(),
            current_edge,
            current_pool.as_deref(),
            deferred_paths.as_mut(),
        )?;
        parent = None;
        current_rule = None;
        current_edge = None;
        current_pool = None;
        deferred_paths = None;
        let line = line.trim_start();

        if let Some(rest) = line.strip_prefix("rule ") {
            let name = rest.trim();
            if !valid_variable_name(name) {
                return Err(
                    Diagnostic::new(path, line_no, 6, "expected rule name").with_source(&raw_line)
                );
            }
            if manifest.scopes[scope].rules.contains_key(name) {
                return Err(Diagnostic::new(
                    path,
                    line_no,
                    line_end_column(&raw_line),
                    format!("duplicate rule '{name}'"),
                )
                .with_source(&raw_line));
            }
            manifest.scopes[scope].rules.insert(
                name.to_owned(),
                Rule {
                    name: name.to_owned(),
                    bindings: HashMap::new(),
                    source: Arc::clone(&source_path),
                    line: line_no,
                },
            );
            parent = Some(Parent::Rule);
            current_rule = Some(name.to_owned());
        } else if let Some(rest) = line.strip_prefix("pool ") {
            let name = rest.trim();
            if !valid_variable_name(name) {
                return Err(
                    Diagnostic::new(path, line_no, 6, "expected pool name").with_source(&raw_line)
                );
            }
            if manifest.pools.contains_key(name) {
                return Err(Diagnostic::new(
                    path,
                    line_no,
                    line_end_column(&raw_line),
                    format!("duplicate pool '{name}'"),
                )
                .with_source(&raw_line));
            }
            manifest.pools.insert(
                name.to_owned(),
                Pool {
                    name: name.to_owned(),
                    depth: 0,
                    depth_specified: false,
                    source: Arc::clone(&source_path),
                    line: line_no,
                },
            );
            parent = Some(Parent::Pool);
            current_pool = Some(name.to_owned());
        } else if let Some(rest) = line.strip_prefix("build ") {
            let (mut edge, deferred) = parse_edge(rest, &source_path, line_no)
                .map_err(|diagnostic| remap_logical_diagnostic(diagnostic, &raw_line, source))?;
            if edge.rule != "phony" && manifest.lookup_rule(scope, &edge.rule).is_none() {
                let column = build_rule_column(&raw_line).unwrap_or(1);
                return Err(diagnostic_for_logical_line(
                    path,
                    line_no,
                    column,
                    format!("unknown build rule '{}'", edge.rule),
                    &raw_line,
                    source,
                ));
            }
            edge.scope = scope;
            manifest.edges.push(edge);
            current_edge = Some(manifest.edges.len() - 1);
            deferred_paths = Some(deferred);
            parent = Some(Parent::Edge);
        } else if let Some(rest) = line.strip_prefix("default ") {
            if let Some(position) = first_unescaped_colon(rest) {
                return Err(Diagnostic::new(
                    path,
                    line_no,
                    "default ".len() + position + 1,
                    "expected newline, got ':'",
                )
                .with_source(&raw_line));
            }
            let mut words = expand_path_words(rest, manifest, scope);
            if words.is_empty() {
                return Err(Diagnostic::new(path, line_no, 9, "expected target name")
                    .with_source(&raw_line));
            }
            if words.iter().any(String::is_empty) {
                return Err(Diagnostic::new(
                    path,
                    line_no,
                    line_end_column(&raw_line),
                    "empty path",
                )
                .with_source(&raw_line));
            }
            for word in &mut words {
                *word = canonicalize_owned_path(std::mem::take(word));
                let known = manifest.edges.iter().any(|edge| {
                    edge.outputs()
                        .chain(edge.inputs())
                        .chain(edge.validations.iter().map(String::as_str))
                        .any(|candidate| candidate == word)
                });
                if !known {
                    return Err(Diagnostic::new(
                        path,
                        line_no,
                        line_end_column(&raw_line),
                        format!("unknown target '{word}'"),
                    )
                    .with_source(&raw_line));
                }
            }
            manifest.defaults.extend(words);
        } else if line == "rule" {
            return Err(
                Diagnostic::new(path, line_no, 5, "expected rule name").with_source(&raw_line)
            );
        } else if line == "pool" {
            return Err(
                Diagnostic::new(path, line_no, 5, "expected pool name").with_source(&raw_line)
            );
        } else if line == "build" {
            return Err(Diagnostic::new(path, line_no, 6, "expected path").with_source(&raw_line));
        } else if line == "default" {
            return Err(
                Diagnostic::new(path, line_no, 8, "expected target name").with_source(&raw_line)
            );
        } else if let Some(rest) = line
            .strip_prefix("include ")
            .or_else(|| (line == "include").then_some(""))
        {
            let include = expand(rest.trim(), |name| {
                manifest.lookup_variable(scope, name).map(str::to_owned)
            });
            if let Some(stack) = stack.as_deref_mut() {
                let child = PathBuf::from(include);
                parse_file_into(&child, manifest, stack, scope).map_err(|error| {
                    contextualize_include_error(error, path, line_no, &raw_line, &child)
                })?;
            }
        } else if let Some(rest) = line
            .strip_prefix("subninja ")
            .or_else(|| (line == "subninja").then_some(""))
        {
            let include = expand(rest.trim(), |name| {
                manifest.lookup_variable(scope, name).map(str::to_owned)
            });
            if let Some(stack) = stack.as_deref_mut() {
                let child = PathBuf::from(include);
                let child_scope = manifest.scopes.len();
                manifest.scopes.push(Scope {
                    parent: Some(scope),
                    ..Scope::default()
                });
                parse_file_into(&child, manifest, stack, child_scope).map_err(|error| {
                    contextualize_include_error(error, path, line_no, &raw_line, &child)
                })?;
            }
        } else if let Some((key, value)) = parse_binding(line) {
            let expanded = expand(value, |name| {
                manifest.lookup_variable(scope, name).map(str::to_owned)
            });
            if key == "ninja_required_version"
                && version_major_minor(SUPPORTED_SYNTAX_VERSION).0
                    > version_major_minor(&expanded).0
            {
                manifest.warnings.push(format!(
                    "Knight syntax version ({SUPPORTED_SYNTAX_VERSION}) is newer than build file ninja_required_version ({expanded}); versions may be incompatible"
                ));
            }
            manifest.scopes[scope]
                .variables
                .insert(key.to_owned(), expanded);
            manifest.has_pool_binding |= key == "pool";
            manifest.has_dyndep_binding |= key == "dyndep";
        } else if let Some((column, token)) = expected_equals_diagnostic(
            line,
            missing_final_newline
                && line_no == source.bytes().filter(|byte| *byte == b'\n').count() + 1,
        ) {
            return Err(Diagnostic::new(
                path,
                line_no,
                column,
                format!("expected '=', got {token}"),
            )
            .with_source(&raw_line));
        } else {
            return Err(
                Diagnostic::new(path, line_no, 1, "expected a declaration").with_source(&raw_line)
            );
        }
    }

    if missing_final_newline {
        let line = source.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let source_line = source.rsplit('\n').next().unwrap_or_default();
        return Err(Diagnostic::new(
            path,
            line,
            source_line.chars().count() + 1,
            "unexpected EOF",
        )
        .with_source(source_line));
    }

    finalize_parent(
        manifest,
        scope,
        parent,
        current_rule.as_deref(),
        current_edge,
        current_pool.as_deref(),
        deferred_paths.as_mut(),
    )?;

    Ok(())
}

fn build_rule_column(line: &str) -> Option<usize> {
    let colon = line.find(':')?;
    let suffix = &line[colon + 1..];
    let whitespace = suffix.len() - suffix.trim_start().len();
    Some(colon + whitespace + 2)
}

fn line_end_column(line: &str) -> usize {
    line.strip_suffix('\r').unwrap_or(line).chars().count() + 1
}

fn expected_equals_diagnostic(line: &str, at_eof: bool) -> Option<(usize, &'static str)> {
    let name_end = line
        .bytes()
        .position(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        .unwrap_or(line.len());
    if name_end == 0 || !valid_variable_name(&line[..name_end]) {
        return None;
    }
    let remainder = &line[name_end..];
    let whitespace = remainder.len() - remainder.trim_start().len();
    let token_start = name_end + whitespace;
    if token_start == line.len() {
        Some((
            line.chars().count() + 1,
            if at_eof { "eof" } else { "newline" },
        ))
    } else if line.as_bytes()[token_start].is_ascii_alphanumeric() {
        Some((token_start + 1, "identifier"))
    } else {
        None
    }
}

fn first_unescaped_colon(input: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' && index + 1 < bytes.len() {
            index += 2;
        } else if bytes[index] == b':' {
            return Some(index);
        } else {
            index += 1;
        }
    }
    None
}

fn contextualize_include_error(
    error: Diagnostic,
    parent: &Path,
    line: usize,
    source_line: &str,
    included: &Path,
) -> Diagnostic {
    let Some(cause) = error.message.strip_prefix("loading manifest: ") else {
        return error;
    };
    let cause = io_error_message(cause);
    let ninja_compat = crate::program_name() == "ninja";
    let mut message = if ninja_compat {
        format!("loading '{}': {cause}", included.display())
    } else {
        format!(
            "loading included manifest '{}': {cause}",
            included.display()
        )
    };
    if ninja_compat && cfg!(windows) {
        // Windows Ninja retains FormatMessage's CRLF, then routes it through
        // its text-mode stderr stream before the lexer adds another newline.
        message.push_str("\r\r\n");
    }
    Diagnostic::new(parent, line, line_end_column(source_line), message).with_source(source_line)
}

fn io_error_message(cause: &str) -> &str {
    cause
        .rsplit_once(" (os error ")
        .and_then(|(message, code)| code.strip_suffix(')').map(|_| message))
        .unwrap_or(cause)
}

fn validate(manifest: &Manifest) -> Result<(), Diagnostic> {
    if let Some(required) = manifest.variables.get("ninja_required_version") {
        if required_version_incompatible(required, SUPPORTED_SYNTAX_VERSION) {
            return Err(Diagnostic::new(
                &manifest.root,
                1,
                1,
                format!(
                    "manifest requires Ninja {required}; Knight supports syntax through 1.14.0"
                ),
            ));
        }
    }
    let mut outputs = HashMap::<&str, &Edge>::new();
    for edge in &manifest.edges {
        for output in edge.outputs() {
            if let Some(previous) = outputs.insert(output, edge) {
                if crate::program_name() == "ninja" {
                    let message = if std::ptr::eq(previous, edge) {
                        format!("{output} is defined as an output multiple times")
                    } else {
                        format!("multiple rules generate {output}")
                    };
                    return Err(Diagnostic::new(
                        edge.source.as_path(),
                        edge.line + edge.bindings.len() + 1,
                        1,
                        message,
                    ));
                }
                return Err(Diagnostic::new(
                    edge.source.as_path(),
                    edge.line,
                    1,
                    format!(
                        "multiple rules generate '{output}' (first at {}:{})",
                        previous.source.display(),
                        previous.line
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn version_major_minor(version: &str) -> (i64, i64) {
    fn component(value: &str, index: usize) -> i64 {
        let part = value.split('.').nth(index).unwrap_or_default();
        let bytes = part.as_bytes();
        let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == 0 || (end == 1 && matches!(bytes.first(), Some(b'+' | b'-'))) {
            0
        } else {
            part[..end].parse().unwrap_or(0)
        }
    }

    (component(version, 0), component(version, 1))
}

fn required_version_incompatible(required: &str, supported: &str) -> bool {
    let (supported_major, supported_minor) = version_major_minor(supported);
    let (required_major, required_minor) = version_major_minor(required);
    supported_major < required_major
        || (supported_major == required_major && supported_minor < required_minor)
}

fn version_supports_newline_escape(version: &str) -> bool {
    let (major, minor) = version_major_minor(version);
    major > 1 || (major == 1 && minor >= 14)
}

fn binding_cycle(manifest: &Manifest, edge: &Edge) -> Option<Vec<String>> {
    fn visit(
        manifest: &Manifest,
        edge: &Edge,
        name: &str,
        stack: &mut Vec<String>,
        resolved: &mut HashSet<String>,
    ) -> Option<Vec<String>> {
        if matches!(name, "in" | "in_newline" | "out") || resolved.contains(name) {
            return None;
        }
        if let Some(start) = stack.iter().position(|entry| entry == name) {
            let mut cycle = stack[start..].to_vec();
            if let Some((first, _)) = cycle.iter().enumerate().min_by_key(|(_, entry)| *entry) {
                cycle.rotate_left(first);
            }
            cycle.push(cycle[0].clone());
            return Some(cycle);
        }
        if edge.bindings.contains_key(name) {
            resolved.insert(name.to_owned());
            return None;
        }
        let Some(raw) = manifest
            .lookup_rule(edge.scope, &edge.rule)
            .and_then(|rule| rule.bindings.get(name))
        else {
            resolved.insert(name.to_owned());
            return None;
        };
        stack.push(name.to_owned());
        for nested in variable_references(raw) {
            if let Some(cycle) = visit(manifest, edge, nested, stack, resolved) {
                return Some(cycle);
            }
        }
        let resolved_name = stack.pop().unwrap();
        resolved.insert(resolved_name);
        None
    }

    let mut names = edge.bindings.keys().map(String::as_str).collect::<Vec<_>>();
    if let Some(rule) = manifest.lookup_rule(edge.scope, &edge.rule) {
        names.extend(rule.bindings.keys().map(String::as_str));
    }
    let mut resolved = HashSet::new();
    for name in names {
        if let Some(cycle) = visit(manifest, edge, name, &mut Vec::new(), &mut resolved) {
            return Some(cycle);
        }
    }
    None
}

fn variable_references(value: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut index = 0;
    let bytes = value.as_bytes();
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        index += 1;
        if index >= bytes.len() {
            break;
        }
        if bytes[index] == b'{' {
            let start = index + 1;
            if let Some(end) = bytes[start..].iter().position(|byte| *byte == b'}') {
                result.push(&value[start..start + end]);
                index = start + end + 1;
            } else {
                break;
            }
        } else if bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'-') {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'-'))
            {
                index += 1;
            }
            result.push(&value[start..index]);
        } else {
            index += 1;
        }
    }
    result
}

fn evaluate_edge_binding(manifest: &Manifest, edge: &Edge, name: &str, depth: usize) -> String {
    if depth > 64 {
        return String::new();
    }
    match name {
        "in" => {
            return join_decanonicalized_paths(
                &edge.explicit_inputs,
                manifest.explicit_input_slash_bits(edge),
                ' ',
            );
        }
        "in_newline" => {
            return join_decanonicalized_paths(
                &edge.explicit_inputs,
                manifest.explicit_input_slash_bits(edge),
                '\n',
            );
        }
        "out" => {
            return join_decanonicalized_paths(
                &edge.explicit_outputs,
                manifest.explicit_output_slash_bits(edge),
                ' ',
            );
        }
        _ => {}
    }
    if let Some(value) = edge.bindings.get(name) {
        return value.clone();
    }
    if let Some(raw) = manifest
        .lookup_rule(edge.scope, &edge.rule)
        .and_then(|rule| rule.bindings.get(name))
    {
        return expand(raw, |nested| {
            Some(evaluate_edge_binding(manifest, edge, nested, depth + 1))
        });
    }
    manifest
        .lookup_variable(edge.scope, name)
        .unwrap_or_default()
        .to_owned()
}

fn join_decanonicalized_paths(paths: &[String], slash_bits: &[u64], separator: char) -> String {
    let capacity = paths.iter().map(String::len).sum::<usize>()
        + paths.len().saturating_sub(1) * separator.len_utf8();
    let mut result = String::with_capacity(capacity);
    for (index, path) in paths.iter().enumerate() {
        if index != 0 {
            result.push(separator);
        }
        result.push_str(&decanonicalize_path(
            path,
            slash_bits.get(index).copied().unwrap_or(0),
        ));
    }
    result
}

fn parse_binding(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if !valid_variable_name(key) {
        return None;
    }
    Some((key, value.trim_start()))
}

#[derive(Debug, PartialEq, Eq)]
enum BuildToken<'a> {
    Word(&'a str, bool),
    Colon,
    Pipe,
    Pipe2,
    Validation,
}

fn lex_build(input: &str) -> Vec<BuildToken<'_>> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        while bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
        {
            index += 1;
        }
        if index == bytes.len() {
            break;
        }
        match bytes[index] {
            b':' => {
                tokens.push(BuildToken::Colon);
                index += 1;
                continue;
            }
            b'|' => {
                let token = if bytes.get(index + 1) == Some(&b'|') {
                    index += 2;
                    BuildToken::Pipe2
                } else if bytes.get(index + 1) == Some(&b'@') {
                    index += 2;
                    BuildToken::Validation
                } else {
                    index += 1;
                    BuildToken::Pipe
                };
                tokens.push(token);
                continue;
            }
            _ => {}
        }

        let start = index;
        while index < bytes.len() {
            match bytes[index] {
                b'$' if index + 1 < bytes.len() => index += 2,
                b' ' | b'\t' | b':' | b'|' => break,
                _ => index += 1,
            }
        }
        let word = &input[start..index];
        tokens.push(BuildToken::Word(
            word,
            word.contains('$') || path_needs_canonicalization(word),
        ));
    }
    tokens
}

fn build_token_offset(input: &str, target: usize) -> usize {
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut token = 0;
    while index < bytes.len() {
        while bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
        {
            index += 1;
        }
        if index == bytes.len() || token == target {
            return index;
        }
        token += 1;
        match bytes[index] {
            b':' => index += 1,
            b'|' if matches!(bytes.get(index + 1), Some(b'|' | b'@')) => index += 2,
            b'|' => index += 1,
            _ => {
                while index < bytes.len() {
                    match bytes[index] {
                        b'$' if index + 1 < bytes.len() => index += 2,
                        b' ' | b'\t' | b':' | b'|' => break,
                        _ => index += 1,
                    }
                }
            }
        }
    }
    input.len()
}

fn path_needs_canonicalization(path: &str) -> bool {
    (cfg!(windows) && path.as_bytes().contains(&b'\\'))
        || path.ends_with('/')
        || path.contains("//")
        || path
            .split('/')
            .any(|component| matches!(component, "." | ".."))
}

fn parse_edge(
    input: &str,
    path: &Arc<PathBuf>,
    line: usize,
) -> Result<(Edge, ParsedEdgePaths), Diagnostic> {
    let tokens = lex_build(input);
    let colon = tokens
        .iter()
        .position(|token| *token == BuildToken::Colon)
        .ok_or_else(|| {
            Diagnostic::new(
                path,
                line,
                6 + input.chars().count() + 1,
                "expected ':', got newline ($ also escapes ':')",
            )
        })?;
    let mut edge = Edge {
        source: Arc::clone(path),
        line,
        ..Edge::default()
    };
    let mut deferred = ParsedEdgePaths::default();
    let mut implicit = false;
    for (token_index, token) in tokens[..colon].iter().enumerate() {
        match token {
            BuildToken::Pipe if !implicit => implicit = true,
            BuildToken::Word(word, needs_canonicalization) => {
                let (value, is_deferred, slash_bits) = if word.contains('$') {
                    ((*word).to_owned(), true, 0)
                } else if *needs_canonicalization {
                    let (path, slash_bits) = canonicalize_owned_path_with_bits((*word).to_owned());
                    (path, false, slash_bits)
                } else {
                    ((*word).to_owned(), false, 0)
                };
                if implicit {
                    let index = edge.implicit_outputs.len();
                    edge.implicit_outputs.push(value);
                    if is_deferred || (cfg!(windows) && slash_bits != 0) {
                        deferred.push(
                            DeferredPathKind::ImplicitOutput,
                            index,
                            is_deferred,
                            slash_bits,
                        );
                    }
                } else {
                    let index = edge.explicit_outputs.len();
                    edge.explicit_outputs.push(value);
                    if is_deferred || (cfg!(windows) && slash_bits != 0) {
                        deferred.push(
                            DeferredPathKind::ExplicitOutput,
                            index,
                            is_deferred,
                            slash_bits,
                        );
                    }
                }
            }
            other => {
                let symbol = match other {
                    BuildToken::Pipe => "'|'",
                    BuildToken::Pipe2 => "'||'",
                    BuildToken::Validation => "'|@'",
                    BuildToken::Colon => "':'",
                    BuildToken::Word(..) => "identifier",
                };
                return Err(Diagnostic::new(
                    path,
                    line,
                    6 + build_token_offset(input, token_index) + 1,
                    format!("expected ':', got {symbol} ($ also escapes ':')"),
                ));
            }
        }
    }
    if edge.explicit_outputs.is_empty() && edge.implicit_outputs.is_empty() {
        return Err(Diagnostic::new(path, line, 1, "build edge has no outputs"));
    }
    let Some(BuildToken::Word(rule, _)) = tokens.get(colon + 1) else {
        let offset = build_token_offset(input, colon + 1);
        return Err(Diagnostic::new(
            path,
            line,
            6 + offset + 1,
            if crate::program_name() == "ninja" {
                "expected build command name"
            } else {
                "expected build rule after ':'"
            },
        ));
    };
    edge.rule = (*rule).to_owned();

    #[derive(Clone, Copy, PartialEq, PartialOrd)]
    enum InputKind {
        Explicit,
        Implicit,
        OrderOnly,
        Validation,
    }
    let mut kind = InputKind::Explicit;
    for (relative_index, token) in tokens[colon + 2..].iter().enumerate() {
        let token_index = colon + 2 + relative_index;
        match token {
            BuildToken::Pipe => {
                if kind >= InputKind::Implicit {
                    return Err(Diagnostic::new(
                        path,
                        line,
                        6 + build_token_offset(input, token_index) + 1,
                        "expected newline, got '|'",
                    ));
                }
                kind = InputKind::Implicit;
            }
            BuildToken::Pipe2 => {
                if kind >= InputKind::OrderOnly {
                    return Err(Diagnostic::new(
                        path,
                        line,
                        6 + build_token_offset(input, token_index) + 1,
                        "expected newline, got '||'",
                    ));
                }
                kind = InputKind::OrderOnly;
            }
            BuildToken::Validation => {
                if kind >= InputKind::Validation {
                    return Err(Diagnostic::new(
                        path,
                        line,
                        6 + build_token_offset(input, token_index) + 1,
                        "expected newline, got '|@'",
                    ));
                }
                kind = InputKind::Validation;
            }
            BuildToken::Word(word, needs_canonicalization) => {
                let (value, is_deferred, slash_bits) = if word.contains('$') {
                    ((*word).to_owned(), true, 0)
                } else if *needs_canonicalization {
                    let (path, slash_bits) = canonicalize_owned_path_with_bits((*word).to_owned());
                    (path, false, slash_bits)
                } else {
                    ((*word).to_owned(), false, 0)
                };
                match kind {
                    InputKind::Explicit => {
                        let index = edge.explicit_inputs.len();
                        edge.explicit_inputs.push(value);
                        if is_deferred || (cfg!(windows) && slash_bits != 0) {
                            deferred.push(
                                DeferredPathKind::ExplicitInput,
                                index,
                                is_deferred,
                                slash_bits,
                            );
                        }
                    }
                    InputKind::Implicit => {
                        let index = edge.implicit_inputs.len();
                        edge.implicit_inputs.push(value);
                        if is_deferred || (cfg!(windows) && slash_bits != 0) {
                            deferred.push(
                                DeferredPathKind::ImplicitInput,
                                index,
                                is_deferred,
                                slash_bits,
                            );
                        }
                    }
                    InputKind::OrderOnly => {
                        let index = edge.order_only_inputs.len();
                        edge.order_only_inputs.push(value);
                        if is_deferred || (cfg!(windows) && slash_bits != 0) {
                            deferred.push(
                                DeferredPathKind::OrderOnlyInput,
                                index,
                                is_deferred,
                                slash_bits,
                            );
                        }
                    }
                    InputKind::Validation => {
                        let index = edge.validations.len();
                        edge.validations.push(value);
                        if is_deferred || (cfg!(windows) && slash_bits != 0) {
                            deferred.push(
                                DeferredPathKind::Validation,
                                index,
                                is_deferred,
                                slash_bits,
                            );
                        }
                    }
                }
            }
            BuildToken::Colon => {
                return Err(Diagnostic::new(
                    path,
                    line,
                    6 + build_token_offset(input, token_index) + 1,
                    "expected newline, got ':'",
                ));
            }
        }
    }
    Ok((edge, deferred))
}

fn expand_path_words(input: &str, manifest: &Manifest, scope: usize) -> Vec<String> {
    split_words(input)
        .into_iter()
        .map(|word| {
            expand(&word, |name| {
                manifest.lookup_variable(scope, name).map(str::to_owned)
            })
        })
        .collect()
}

fn split_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            word.push(c);
            if let Some(next) = chars.next() {
                word.push(next);
            }
        } else if c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

pub fn expand(mut input: &str, mut lookup: impl FnMut(&str) -> Option<String>) -> String {
    let mut output = String::with_capacity(input.len());
    while let Some(pos) = input.find('$') {
        output.push_str(&input[..pos]);
        input = &input[pos + 1..];
        let Some(first) = input.chars().next() else {
            output.push('$');
            break;
        };
        match first {
            '$' => {
                output.push('$');
                input = &input[1..];
            }
            ' ' => {
                output.push(' ');
                input = &input[1..];
            }
            ':' => {
                output.push(':');
                input = &input[1..];
            }
            '^' => {
                output.push('\n');
                input = &input[1..];
            }
            '{' => {
                if let Some(end) = input.find('}') {
                    let name = &input[1..end];
                    if let Some(value) = lookup(name) {
                        output.push_str(&value);
                    }
                    input = &input[end + 1..];
                } else {
                    output.push_str("${");
                    input = &input[1..];
                }
            }
            _ => {
                let end = input
                    .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
                    .unwrap_or(input.len());
                if end == 0 {
                    output.push('$');
                    output.push(first);
                    input = &input[first.len_utf8()..];
                    continue;
                }
                let name = &input[..end];
                if let Some(value) = lookup(name) {
                    output.push_str(&value);
                }
                input = &input[end..];
            }
        }
    }
    output.push_str(input);
    output
}

fn dollar_escape_issues(input: &str) -> (Option<usize>, Option<usize>) {
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut newline_escape = None;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let Some(next) = bytes.get(index).copied() else {
            return (Some(start), newline_escape);
        };
        match next {
            b'$' | b' ' | b':' => index += 1,
            b'^' => {
                newline_escape.get_or_insert(start);
                index += 1;
            }
            b'{' => {
                let Some(end) = input[index + 1..].find('}') else {
                    return (Some(start), newline_escape);
                };
                if end == 0 || !valid_variable_name(&input[index + 1..index + 1 + end]) {
                    return (Some(start), newline_escape);
                }
                index += end + 2;
            }
            byte if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') => {
                index += 1;
                while bytes.get(index).is_some_and(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-')
                }) {
                    index += 1;
                }
            }
            _ => return (Some(start), newline_escape),
        }
    }
    (None, newline_escape)
}

fn valid_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

pub fn canonicalize_path(path: &str) -> String {
    canonicalize_owned_path(path.to_owned())
}

pub fn unknown_target_message(manifest: &Manifest, target: &str) -> String {
    let mut message = format!("unknown target '{target}'");
    if target == "clean" {
        message.push_str(", did you mean 'ninja -t clean'?");
        return message;
    }
    if target == "help" {
        message.push_str(", did you mean 'ninja -h'?");
        return message;
    }
    let suggestion = spellcheck(
        target,
        manifest.edges.iter().flat_map(|edge| {
            edge.outputs()
                .chain(edge.inputs())
                .chain(edge.validations.iter().map(String::as_str))
        }),
    );
    if let Some(suggestion) = suggestion {
        message.push_str(&format!(", did you mean '{suggestion}'?"));
    }
    message
}

pub fn spellcheck<'a>(
    text: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut best = None;
    let mut best_distance = 4;
    for candidate in candidates {
        let distance = edit_distance(candidate.as_bytes(), text.as_bytes(), 3);
        // Ninja retains the first candidate at the minimum edit distance.
        if distance < best_distance {
            best = Some(candidate);
            best_distance = distance;
        }
    }
    best
}

fn edit_distance(left: &[u8], right: &[u8], maximum: usize) -> usize {
    edit_distance_with_replacements(left, right, true, maximum)
}

fn edit_distance_with_replacements(
    left: &[u8],
    right: &[u8],
    allow_replacements: bool,
    maximum: usize,
) -> usize {
    let mut row = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_byte) in left.iter().enumerate() {
        let mut previous = left_index;
        row[0] = left_index + 1;
        let mut best = row[0];
        for (right_index, right_byte) in right.iter().enumerate() {
            let old = row[right_index + 1];
            let replacement = if left_byte == right_byte {
                previous
            } else if allow_replacements {
                previous + 1
            } else {
                maximum.saturating_add(1)
            };
            row[right_index + 1] = replacement.min(row[right_index].min(old) + 1);
            previous = old;
            best = best.min(row[right_index + 1]);
        }
        if best > maximum {
            return maximum.saturating_add(1);
        }
    }
    row[right.len()]
}

pub(crate) fn decanonicalize_path(path: &str, slash_bits: u64) -> Cow<'_, str> {
    #[cfg(windows)]
    {
        if slash_bits == 0 {
            return Cow::Borrowed(path);
        }
        let mut bytes = path.as_bytes().to_owned();
        let mut mask = 1u64;
        for byte in &mut bytes {
            if *byte == b'/' {
                if slash_bits & mask != 0 {
                    *byte = b'\\';
                }
                mask <<= 1;
            }
        }
        Cow::Owned(String::from_utf8(bytes).expect("separator replacement preserves UTF-8"))
    }
    #[cfg(not(windows))]
    {
        let _ = slash_bits;
        Cow::Borrowed(path)
    }
}

pub fn canonicalize_owned_path(path: String) -> String {
    canonicalize_owned_path_with_bits(path).0
}

fn canonicalize_owned_path_with_bits(path: String) -> (String, u64) {
    if path.is_empty() {
        return (path, 0);
    }
    let has_platform_separator = cfg!(windows) && path.as_bytes().contains(&b'\\');
    if !has_platform_separator
        && !path.ends_with('/')
        && !path.contains("//")
        && !path
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return (path, 0);
    }

    let is_separator = |byte| byte == b'/' || (cfg!(windows) && byte == b'\\');
    let mut bytes = path.into_bytes();
    let end = bytes.len();
    let mut source = 0usize;
    let mut destination = 0usize;
    let mut destination_start = 0usize;
    if is_separator(bytes[0]) {
        if cfg!(windows) && end >= 2 && is_separator(bytes[1]) {
            source = 2;
            destination = 2;
        } else {
            source = 1;
            destination = 1;
        }
        destination_start = destination;
    } else {
        while source + 3 <= end
            && bytes[source] == b'.'
            && bytes[source + 1] == b'.'
            && is_separator(bytes[source + 2])
        {
            source += 3;
            destination += 3;
        }
    }

    let destination_base = destination;
    let mut component_count = 0usize;
    while source < end {
        let Some(next_separator) = (source..end).find(|index| is_separator(bytes[*index])) else {
            break;
        };
        let source_next = next_separator + 1;
        let component_len = next_separator - source;
        if component_len <= 2 {
            if component_len == 0 {
                source = source_next;
                continue;
            }
            if bytes[source] == b'.' {
                if component_len == 1 {
                    source = source_next;
                    continue;
                }
                if bytes[source + 1] == b'.' {
                    if component_count > 0 {
                        component_count -= 1;
                        destination -= 1;
                        while destination > destination_base
                            && !is_separator(bytes[destination - 1])
                        {
                            destination -= 1;
                        }
                    } else {
                        bytes[destination] = b'.';
                        bytes[destination + 1] = b'.';
                        bytes[destination + 2] = bytes[source + 2];
                        destination += 3;
                    }
                    source = source_next;
                    continue;
                }
            }
        }
        component_count += 1;
        if destination != source {
            bytes.copy_within(source..source_next, destination);
        }
        destination += source_next - source;
        source = source_next;
    }

    let component_len = end - source;
    if component_len != 0 {
        if bytes[source] == b'.' {
            if component_len == 2 && bytes[source + 1] == b'.' {
                if component_count > 0 {
                    destination -= 1;
                    while destination > destination_base && !is_separator(bytes[destination - 1]) {
                        destination -= 1;
                    }
                } else {
                    bytes[destination] = b'.';
                    bytes[destination + 1] = b'.';
                    destination += 2;
                }
            } else if component_len != 1 {
                if destination != source {
                    bytes.copy_within(source..end, destination);
                }
                destination += component_len;
            }
        } else {
            if destination != source {
                bytes.copy_within(source..end, destination);
            }
            destination += component_len;
        }
    }

    if destination > destination_start && is_separator(bytes[destination - 1]) {
        destination -= 1;
    }
    if destination == 0 {
        bytes[0] = b'.';
        destination = 1;
    }
    bytes.truncate(destination);

    let mut slash_bits = 0u64;
    if cfg!(windows) {
        let mut mask = 1u64;
        for byte in &mut bytes {
            if *byte == b'\\' {
                slash_bits |= mask;
                *byte = b'/';
            }
            if *byte == b'/' {
                mask <<= 1;
            }
        }
    }
    (
        String::from_utf8(bytes).expect("canonicalization preserves UTF-8 boundaries"),
        slash_bits,
    )
}

struct LogicalLines<'a> {
    lines: std::str::Lines<'a>,
    next_line: usize,
}

fn logical_lines(source: &str) -> LogicalLines<'_> {
    LogicalLines {
        lines: source.lines(),
        next_line: 1,
    }
}

impl<'a> Iterator for LogicalLines<'a> {
    type Item = (usize, Cow<'a, str>);

    fn next(&mut self) -> Option<Self::Item> {
        let start_line = self.next_line;
        let first = self.lines.next()?;
        self.next_line += 1;
        if first
            .as_bytes()
            .iter()
            .rev()
            .take_while(|byte| **byte == b'$')
            .count()
            % 2
            == 0
        {
            return Some((start_line, Cow::Borrowed(first)));
        }

        let mut buffer = first[..first.len() - 1].to_owned();
        let mut continued_at_eof = true;
        for line in self.lines.by_ref() {
            self.next_line += 1;
            let trimmed = line.trim_start();
            buffer.push_str(trimmed);
            let continued = buffer
                .as_bytes()
                .iter()
                .rev()
                .take_while(|byte| **byte == b'$')
                .count()
                % 2
                == 1;
            if continued {
                buffer.pop();
            } else {
                continued_at_eof = false;
                break;
            }
        }
        if continued_at_eof {
            buffer.push('$');
        }
        Some((start_line, Cow::Owned(buffer)))
    }
}

fn diagnostic_for_logical_line(
    path: &Path,
    line: usize,
    column: usize,
    message: impl Into<String>,
    logical_source: &str,
    complete_source: &str,
) -> Diagnostic {
    remap_logical_diagnostic(
        Diagnostic::new(path, line, column, message),
        logical_source,
        complete_source,
    )
}

fn remap_logical_diagnostic(
    mut diagnostic: Diagnostic,
    logical_source: &str,
    complete_source: &str,
) -> Diagnostic {
    let logical_offset = diagnostic.column.saturating_sub(1);
    let mut logical_start = 0;
    for (physical_index, physical_source) in complete_source
        .split_terminator('\n')
        .skip(diagnostic.line.saturating_sub(1))
        .enumerate()
    {
        let source = physical_source
            .strip_suffix('\r')
            .unwrap_or(physical_source);
        let source_start = if physical_index == 0 {
            0
        } else {
            source.len() - source.trim_start().len()
        };
        let content = &source[source_start..];
        let continued = content
            .as_bytes()
            .iter()
            .rev()
            .take_while(|byte| **byte == b'$')
            .count()
            % 2
            == 1;
        let logical_end = logical_start + content.len().saturating_sub(usize::from(continued));
        if logical_offset < logical_end || !continued {
            diagnostic.line += physical_index;
            diagnostic.column = source_start + logical_offset - logical_start + 1;
            diagnostic.source_line = Some(physical_source.to_owned());
            return diagnostic;
        }
        logical_start = logical_end;
    }
    diagnostic.with_source(logical_source)
}

fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        ""
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upstream_disk_interface_read_file_case() {
        let temp = tempdir().unwrap();
        let missing = temp.path().join("foobar");
        let error = load_manifest(&missing).unwrap_err();
        assert!(error.message.starts_with("loading manifest: "));

        let manifest = temp.path().join("testfile");
        fs::write(
            &manifest,
            "value = test content\nbuild ok: phony\ndefault ok\n",
        )
        .unwrap();
        let parsed = load_manifest(&manifest).unwrap();
        assert_eq!(parsed.variables["value"], "test content");
        assert_eq!(parsed.edges[0].explicit_outputs, ["ok"]);
    }

    #[test]
    fn detects_include_cycles_through_hard_links() {
        let temp = tempdir().unwrap();
        let manifest = temp.path().join("build.ninja");
        let alias = temp.path().join("alias.ninja");
        fs::write(&manifest, format!("include {}\n", alias.display())).unwrap();
        fs::hard_link(&manifest, alias).unwrap();

        let error = load_manifest(&manifest).unwrap_err();
        assert_eq!(error.message, "include cycle detected");
    }

    #[test]
    fn parses_core_manifest_syntax() {
        let source = r#"
cc = clang
rule compile
  command = $cc -c $in -o $out
  description = CC $out
pool link_pool
  depth = 1
build obj/foo$ bar.o | obj/foo.d: compile src/foo$ bar.c | config.h || generated.h |@ lint
  flags = -O2
default obj/foo$ bar.o
"#;
        let manifest = parse_manifest(source, "build.ninja").unwrap();
        assert_eq!(
            manifest.rules["compile"].bindings["command"],
            "$cc -c $in -o $out"
        );
        assert_eq!(manifest.pools["link_pool"].depth, 1);
        let edge = &manifest.edges[0];
        assert_eq!(edge.explicit_outputs, ["obj/foo bar.o"]);
        assert_eq!(edge.implicit_outputs, ["obj/foo.d"]);
        assert_eq!(edge.explicit_inputs, ["src/foo bar.c"]);
        assert_eq!(edge.implicit_inputs, ["config.h"]);
        assert_eq!(edge.order_only_inputs, ["generated.h"]);
        assert_eq!(edge.validations, ["lint"]);
    }

    #[test]
    fn upstream_manifest_parser_semantic_corpus() {
        let manifest = parse_manifest(
            concat!(
                "rule cat\n",
                "  command = cat $in > $out\n",
                "  depfile = deps.d\n",
                "  deps = gcc\n",
                "  description = compile\n",
                "  generator = 1\n",
                "  restat = 1\n",
                "  rspfile = args.rsp\n",
                "  rspfile_content = $in\n",
                "build result: cat in_1.cc in-2.O\n",
            ),
            "build.ninja",
        )
        .unwrap();
        let rule = &manifest.rules["cat"];
        assert_eq!(rule.name, "cat");
        assert_eq!(rule.bindings.len(), 8);
        assert_eq!(rule.bindings["command"], "cat $in > $out");
        assert_eq!(rule.bindings["rspfile_content"], "$in");
        assert_eq!(manifest.edges[0].explicit_inputs, ["in_1.cc", "in-2.O"]);

        for (name, binding) in [("depfile", "depfile = deps.d"), ("deps", "deps = gcc")] {
            let source = format!("rule cc\n  command = cc\n  {binding}\nbuild a.o b.o: cc c.cc\n");
            let manifest = parse_manifest(&source, "build.ninja").unwrap();
            assert_eq!(
                manifest.edges[0].explicit_outputs,
                ["a.o", "b.o"],
                "case={name}"
            );
        }

        let manifest = parse_manifest(
            concat!(
                "l = one-letter-test\n",
                "rule link\n",
                "  command = ld $l $extra $with_under -o $out $in\n",
                "extra = -pthread\n",
                "with_under = -under\n",
                "build a: link b c\n",
                "nested1 = 1\n",
                "nested2 = $nested1/2\n",
                "build supernested: link x\n",
                "  extra = $nested2/3\n",
            ),
            "build.ninja",
        )
        .unwrap();
        assert_eq!(
            evaluate_edge_binding(&manifest, &manifest.edges[0], "command", 0),
            "ld one-letter-test -pthread -under -o a b c"
        );
        assert_eq!(manifest.lookup_variable(0, "nested2"), Some("1/2"));
        assert_eq!(
            evaluate_edge_binding(&manifest, &manifest.edges[1], "command", 0),
            "ld one-letter-test 1/2/3 -under -o supernested x"
        );

        let manifest = parse_manifest(
            concat!(
                "foo = bar\n",
                "rule cmd\n",
                "  command = cmd $foo $in $out\n",
                "build inner: cmd a\n",
                "  foo = baz\n",
                "build outer: cmd b\n",
            ),
            "build.ninja",
        )
        .unwrap();
        assert_eq!(
            evaluate_edge_binding(&manifest, &manifest.edges[0], "command", 0),
            "cmd baz a inner"
        );
        assert_eq!(
            evaluate_edge_binding(&manifest, &manifest.edges[1], "command", 0),
            "cmd bar b outer"
        );

        let manifest = parse_manifest(
            concat!(
                "backslash = bar\\baz\n",
                "backslash_space = bar\\ baz\n",
                "hash = not # a comment\n",
                "rule escaped\n",
                "  command = ${out}bar$$baz$$$\n",
                "blah\n",
                "x = $$dollar\n",
                "build $x: escaped y\n",
            ),
            "build.ninja",
        )
        .unwrap();
        assert_eq!(manifest.variables["backslash"], "bar\\baz");
        assert_eq!(manifest.variables["backslash_space"], "bar\\ baz");
        assert_eq!(manifest.variables["hash"], "not # a comment");
        assert_eq!(manifest.variables["x"], "$dollar");
        assert_eq!(manifest.edges[0].explicit_outputs, ["$dollar"]);
        assert_eq!(
            evaluate_edge_binding(&manifest, &manifest.edges[0], "command", 0),
            "$dollarbar$baz$blah"
        );

        let manifest = parse_manifest(
            concat!(
                "rule cat\n",
                "  command = cat $in_newline > $out\n",
                "dir = out\n",
                "build $dir/exe: cat ./bar/baz/../foo.cc second.cc\n",
            ),
            "build.ninja",
        )
        .unwrap();
        assert_eq!(manifest.edges[0].explicit_outputs, ["out/exe"]);
        assert_eq!(
            manifest.edges[0].explicit_inputs,
            ["bar/foo.cc", "second.cc"]
        );
        assert_eq!(
            evaluate_edge_binding(&manifest, &manifest.edges[0], "command", 0),
            "cat bar/foo.cc\nsecond.cc > out/exe"
        );

        let manifest = parse_manifest(
            concat!(
                "rule cat\n  command = cat\n",
                "build explicit | implicit: cat in | implicit-in || order |@ validation\n",
                "build | only-implicit: cat\n",
                "default explicit only-implicit\n",
            ),
            "build.ninja",
        )
        .unwrap();
        let edge = &manifest.edges[0];
        assert_eq!(edge.explicit_outputs, ["explicit"]);
        assert_eq!(edge.implicit_outputs, ["implicit"]);
        assert_eq!(edge.explicit_inputs, ["in"]);
        assert_eq!(edge.implicit_inputs, ["implicit-in"]);
        assert_eq!(edge.order_only_inputs, ["order"]);
        assert_eq!(edge.validations, ["validation"]);
        assert!(manifest.edges[1].explicit_outputs.is_empty());
        assert_eq!(manifest.edges[1].implicit_outputs, ["only-implicit"]);
        assert_eq!(manifest.defaults, ["explicit", "only-implicit"]);
        let empty_implicit = parse_manifest(
            "rule cat\n  command = cat\nbuild explicit | : cat\n",
            "build.ninja",
        )
        .unwrap();
        assert_eq!(empty_implicit.edges[0].explicit_outputs, ["explicit"]);
        assert!(empty_implicit.edges[0].implicit_outputs.is_empty());

        for (name, source, expected) in [
            (
                "explicit",
                "rule cat\n  command = cat\nbuild result: cat dd\n  dyndep = dd\n",
                "dd",
            ),
            (
                "implicit",
                "rule cat\n  command = cat\nbuild result: cat in | dd\n  dyndep = dd\n",
                "dd",
            ),
            (
                "order-only",
                "rule cat\n  command = cat\nbuild result: cat in || dd\n  dyndep = dd\n",
                "dd",
            ),
            (
                "rule",
                "rule cat\n  command = cat\n  dyndep = $in\nbuild result: cat dd\n",
                "dd",
            ),
        ] {
            let manifest = parse_manifest(source, "build.ninja").unwrap();
            assert_eq!(
                evaluate_edge_binding(&manifest, &manifest.edges[0], "dyndep", 0),
                expected,
                "case={name}"
            );
        }
        let no_dyndep = parse_manifest("build result: phony\n", "build.ninja").unwrap();
        assert_eq!(
            evaluate_edge_binding(&no_dyndep, &no_dyndep.edges[0], "dyndep", 0),
            ""
        );

        let utf8_and_crlf = parse_manifest(
            concat!(
                "# comment\r\nrule utf8\r\n  command = true\r\n  description = compilaci",
                "\u{00f3}\r\n",
            ),
            "build.ninja",
        )
        .unwrap();
        assert_eq!(
            utf8_and_crlf.rules["utf8"].bindings["description"],
            "compilaci\u{00f3}"
        );
    }

    #[test]
    fn upstream_manifest_parser_case_inventory_is_complete() {
        const CASES: [&str; 53] = [
            "Empty",
            "Rules",
            "RuleAttributes",
            "IgnoreIndentedComments",
            "IgnoreIndentedBlankLines",
            "ResponseFiles",
            "InNewline",
            "Variables",
            "VariableScope",
            "Continuation",
            "Backslash",
            "Comment",
            "Dollars",
            "EscapeSpaces",
            "CanonicalizeFile",
            "CanonicalizeFileBackslashes",
            "PathVariables",
            "CanonicalizePaths",
            "CanonicalizePathsBackslashes",
            "DuplicateEdgeWithMultipleOutputsError",
            "DuplicateEdgeInIncludedFile",
            "PhonySelfReferenceIgnored",
            "PhonySelfReferenceKept",
            "ReservedWords",
            "Errors",
            "MissingInput",
            "MultipleOutputs",
            "MultipleOutputsWithDeps",
            "SubNinja",
            "MissingSubNinja",
            "DuplicateRuleInDifferentSubninjas",
            "DuplicateRuleInDifferentSubninjasWithInclude",
            "Include",
            "BrokenInclude",
            "Implicit",
            "OrderOnly",
            "Validations",
            "ImplicitOutput",
            "ImplicitOutputEmpty",
            "ImplicitOutputDupeError",
            "ImplicitOutputDupesError",
            "NoExplicitOutput",
            "DefaultDefault",
            "DefaultDefaultCycle",
            "DefaultStatements",
            "UTF8",
            "CRLF",
            "DyndepNotSpecified",
            "DyndepNotInput",
            "DyndepExplicitInput",
            "DyndepImplicitInput",
            "DyndepOrderOnlyInput",
            "DyndepRuleInput",
        ];
        let unique = CASES.into_iter().collect::<HashSet<_>>();
        assert_eq!(unique.len(), CASES.len());
    }

    #[test]
    fn tracks_whether_dependency_metadata_can_be_used() {
        let plain = parse_manifest("build out: phony\n", "build.ninja").unwrap();
        assert!(!plain.has_dependency_bindings());

        let rule_binding = parse_manifest(
            "rule cc\n  command = cc\n  deps = gcc\nbuild out: cc\n",
            "build.ninja",
        )
        .unwrap();
        assert!(rule_binding.has_dependency_bindings());

        let edge_binding = parse_manifest(
            "rule cc\n  command = cc\nbuild out: cc\n  depfile = out.d\n",
            "build.ninja",
        )
        .unwrap();
        assert!(edge_binding.has_dependency_bindings());
    }

    #[test]
    fn expansion_handles_ninja_escapes() {
        let vars = [
            ("name".to_owned(), "world".to_owned()),
            ("with-dash".to_owned(), "dash".to_owned()),
            ("with.dot".to_owned(), "dot".to_owned()),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();
        assert_eq!(
            expand(
                "hello $name $with-dash ${with.dot} $$ $: ${missing}",
                |name| vars.get(name).cloned()
            ),
            "hello world dash dot $ : "
        );
        assert_eq!(
            parse_manifest("value = $^\n", "build.ninja")
                .unwrap_err()
                .message,
            "using $^ escape requires specifying 'ninja_required_version' with version greater or equal 1.14"
        );
        let manifest = parse_manifest(
            "ninja_required_version = 1.14\nvalue = one$^two\n",
            "build.ninja",
        )
        .unwrap();
        assert_eq!(manifest.variables["value"], "one\ntwo");
    }

    #[test]
    fn hash_is_only_a_comment_at_the_start_of_a_logical_line() {
        let manifest = parse_manifest(
            "# comment\n  # indented\nvalue = not # a comment\n",
            "build.ninja",
        )
        .unwrap();
        assert_eq!(manifest.variables["value"], "not # a comment");
    }

    #[test]
    fn requires_a_final_newline_like_ninja() {
        let error = parse_manifest("value = complete", "build.ninja").unwrap_err();
        assert_eq!(error.message, "unexpected EOF");
        assert_eq!(error.column, 17);
    }

    #[test]
    fn canonicalizes_ninja_path_identity() {
        for (input, expected) in [
            ("", ""),
            (".", "."),
            ("./foo/./bar", "foo/bar"),
            ("foo//bar/..", "foo"),
            ("foo/..", "."),
            ("./x/../foo/../../bar.h", "../bar.h"),
            ("/foo/..", "/"),
            ("/../..", "/../.."),
            ("../../foo/bar.h", "../../foo/bar.h"),
        ] {
            assert_eq!(canonicalize_path(input), expected, "input={input}");
        }
        #[cfg(windows)]
        for (input, expected) in [
            (r".\foo\.\bar.h", "foo/bar.h"),
            (r"foo\\.\\..\\\bar", "bar"),
            (r"\\server\share\file", "//server/share/file"),
            (r"C:\foo\..\bar", "C:/bar"),
        ] {
            assert_eq!(canonicalize_path(input), expected, "input={input}");
        }
    }

    #[test]
    fn upstream_canonicalize_path_corpus() {
        for (input, expected) in [
            ("", ""),
            ("foo.h", "foo.h"),
            ("./foo.h", "foo.h"),
            ("./foo/./bar.h", "foo/bar.h"),
            ("./x/foo/../bar.h", "x/bar.h"),
            ("./x/foo/../../bar.h", "bar.h"),
            ("foo//bar", "foo/bar"),
            ("foo//.//..///bar", "bar"),
            ("./x/../foo/../../bar.h", "../bar.h"),
            ("foo/./.", "foo"),
            ("foo/bar/..", "foo"),
            ("foo/.hidden_bar", "foo/.hidden_bar"),
            ("/foo", "/foo"),
            ("//foo", if cfg!(windows) { "//foo" } else { "/foo" }),
            ("..", ".."),
            ("../", ".."),
            ("../foo", "../foo"),
            ("../foo/", "../foo"),
            ("../..", "../.."),
            ("../../", "../.."),
            ("./../", ".."),
            ("/..", "/.."),
            ("/../", "/.."),
            ("/../..", "/../.."),
            ("/../../", "/../.."),
            ("/", "/"),
            ("/foo/..", "/"),
            (".", "."),
            ("./.", "."),
            ("foo/..", "."),
            ("foo/.._bar", "foo/.._bar"),
            ("../../foo/bar.h", "../../foo/bar.h"),
            ("test/../../foo/bar.h", "../foo/bar.h"),
            ("/usr/include/stdio.h", "/usr/include/stdio.h"),
        ] {
            assert_eq!(canonicalize_path(input), expected, "input={input}");
        }
    }

    #[test]
    fn upstream_canonicalize_path_length_and_component_corpus() {
        let source = "foo/. bar/.";
        assert_eq!(canonicalize_path(&source[..5]), "foo");
        assert_eq!(source, "foo/. bar/.");
        let source = "foo/../file bar/.";
        assert_eq!(canonicalize_path(&source[..11]), "file");
        assert_eq!(source, "foo/../file bar/.");

        let many = std::iter::repeat_n("a", 219)
            .chain(["x", "y.h"])
            .collect::<Vec<_>>()
            .join(if cfg!(windows) { r"\" } else { "/" });
        let canonical = canonicalize_path(&many);
        assert_eq!(canonical.matches('/').count(), 220);
        assert!(canonical.ends_with("x/y.h"));

        let dotted = std::iter::repeat_n(["a", "."], 32)
            .flatten()
            .chain(["x.h"])
            .collect::<Vec<_>>()
            .join(if cfg!(windows) { r"\" } else { "/" });
        let canonical = canonicalize_path(&dotted);
        assert_eq!(canonical.matches('/').count(), 32);
        assert!(canonical.ends_with("a/x.h"));
    }

    #[cfg(windows)]
    #[test]
    fn upstream_windows_slash_tracking_corpus() {
        for (input, expected, slash_bits) in [
            (r"foo.h", "foo.h", 0),
            (r"a\foo.h", "a/foo.h", 1),
            (r"a/bcd/efh\foo.h", "a/bcd/efh/foo.h", 4),
            (r"a\bcd/efh\foo.h", "a/bcd/efh/foo.h", 5),
            (r"a\bcd\efh\foo.h", "a/bcd/efh/foo.h", 7),
            (r"a/bcd/efh/foo.h", "a/bcd/efh/foo.h", 0),
            (r"a\./efh\foo.h", "a/efh/foo.h", 3),
            (r"a\../efh\foo.h", "efh/foo.h", 1),
            (r"a\b\c\d\e\f\g\foo.h", "a/b/c/d/e/f/g/foo.h", 127),
            (r"a\b\c\..\..\..\g\foo.h", "g/foo.h", 1),
            (r"a\b/c\../../..\g\foo.h", "g/foo.h", 1),
            (r"a\b/c\./../..\g\foo.h", "a/g/foo.h", 3),
            (r"a\b/c\./../..\g/foo.h", "a/g/foo.h", 1),
            (r"a\\\foo.h", "a/foo.h", 1),
            (r"a/\\foo.h", "a/foo.h", 0),
            (r"a\//foo.h", "a/foo.h", 1),
        ] {
            assert_eq!(
                canonicalize_owned_path_with_bits(input.to_owned()),
                (expected.to_owned(), slash_bits),
                "input={input}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn upstream_windows_canonicalize_path_corpus() {
        for (input, expected) in [
            (r"", ""),
            (r"foo.h", "foo.h"),
            (r".\foo.h", "foo.h"),
            (r".\foo\.\bar.h", "foo/bar.h"),
            (r".\x\foo\..\bar.h", "x/bar.h"),
            (r".\x\foo\..\..\bar.h", "bar.h"),
            (r"foo\\bar", "foo/bar"),
            (r"foo\\.\\..\\\bar", "bar"),
            (r".\x\..\foo\..\..\bar.h", "../bar.h"),
            (r"foo\.\.", "foo"),
            (r"foo\bar\..", "foo"),
            (r"foo\.hidden_bar", "foo/.hidden_bar"),
            (r"\foo", "/foo"),
            (r"\\foo", "//foo"),
            (r"\", "/"),
        ] {
            assert_eq!(canonicalize_path(input), expected, "input={input}");
        }

        let path = std::iter::repeat_n("a", 220)
            .chain(std::iter::once("x.h"))
            .collect::<Vec<_>>()
            .join(r"\");
        let (canonical, slash_bits) = canonicalize_owned_path_with_bits(path);
        assert_eq!(canonical.matches('/').count(), 220);
        assert_eq!(slash_bits, u64::MAX);
    }

    #[cfg(windows)]
    #[test]
    fn preserves_ninja_first_node_separator_spelling() {
        let manifest = parse_manifest(
            "rule echo\n  command = echo $in $out\n\
             build seed: phony dir/file\n\
             build out\\mixed/path\\file: echo dir\\file\n",
            "build.ninja",
        )
        .unwrap();
        let edge = &manifest.edges[1];
        assert_eq!(manifest.explicit_output_slash_bits(edge), &[5]);
        assert_eq!(manifest.explicit_input_slash_bits(edge), &[0]);
        assert_eq!(
            evaluate_edge_binding(&manifest, edge, "out", 0),
            r"out\mixed/path\file"
        );
        assert_eq!(evaluate_edge_binding(&manifest, edge, "in", 0), "dir/file");
    }

    #[test]
    fn rejects_duplicate_outputs_after_canonicalization() {
        let error = parse_manifest(
            "build ./out: phony\nbuild dir/../out: phony\n",
            "build.ninja",
        )
        .unwrap_err();
        assert!(error.message.contains("multiple rules generate 'out'"));
    }

    #[test]
    fn validates_rule_bindings_and_response_file_pairs() {
        parse_manifest(
            "rule cc\n  command = cc\n  dyndep = $out.dd\nbuild out: cc || out.dd\n",
            "build.ninja",
        )
        .unwrap();
        let dyndep = parse_manifest(
            "rule cc\n  command = cc\n  dyndep = missing\nbuild out: cc input\n",
            "build.ninja",
        )
        .unwrap_err();
        assert_eq!(dyndep.message, "dyndep 'missing' is not an input");
        let unknown = parse_manifest(
            "rule cc\n  command = cc\n  arbitrary = value\n",
            "build.ninja",
        )
        .unwrap_err();
        assert!(unknown.message.contains("unexpected variable 'arbitrary'"));

        let rsp = parse_manifest(
            "rule cc\n  command = cc\n  rspfile = args.rsp\n",
            "build.ninja",
        )
        .unwrap_err();
        assert_eq!(
            rsp.message,
            "rspfile and rspfile_content need to be both specified"
        );

        let dangling = parse_manifest("value = $", "build.ninja").unwrap_err();
        assert_eq!(
            dangling.message,
            "bad $-escape (literal $ must be written as $$)"
        );

        let empty_default = parse_manifest("default $missing\n", "build.ninja").unwrap_err();
        assert_eq!(empty_default.message, "empty path");
    }

    #[test]
    fn logical_line_continuation_strips_leading_space_via_binding_trim() {
        let manifest = parse_manifest("x = one $\n    two\n", "build.ninja").unwrap();
        assert_eq!(manifest.variables["x"], "one two");
    }

    #[test]
    fn blank_lines_end_binding_blocks_but_comments_do_not() {
        parse_manifest(
            "rule okay\n  command = okay\n  # comment\n  generator = 1\n",
            "build.ninja",
        )
        .unwrap();
        let error = parse_manifest(
            "rule broken\n  command = broken\n  \n  generator = 1\n",
            "build.ninja",
        )
        .unwrap_err();
        assert_eq!(error.message, "unexpected indentation");
    }

    #[test]
    fn rejects_tab_indentation() {
        let error = parse_manifest("rule cc\n\tcommand = echo\nbuild out: cc\n", "build.ninja")
            .unwrap_err();
        assert_eq!(error.line, 2);
        assert_eq!(error.column, 1);
        assert_eq!(error.message, "tabs are not allowed, use spaces");
    }

    #[test]
    fn rejects_duplicate_outputs() {
        let error = parse_manifest("build x: phony\nbuild x: phony\n", "b.ninja").unwrap_err();
        assert!(error.message.contains("multiple rules"));
    }

    #[test]
    fn permits_edges_with_only_implicit_outputs() {
        let manifest = parse_manifest(
            "rule generate\n  command = generate\nbuild | implicit: generate\ndefault implicit\n",
            "build.ninja",
        )
        .unwrap();
        assert!(manifest.edges[0].explicit_outputs.is_empty());
        assert_eq!(manifest.edges[0].implicit_outputs, ["implicit"]);
    }

    #[test]
    fn rejects_unknown_evaluated_pool_names() {
        let error = parse_manifest(
            "rule cc\n  command = cc\n  pool = $selected_pool\nbuild out: cc\n  selected_pool = missing\n",
            "b.ninja",
        )
        .unwrap_err();
        assert_eq!(error.message, "unknown pool name 'missing'");
    }

    #[test]
    fn diagnoses_rule_binding_cycles() {
        let error = parse_manifest(
            "rule cc\n  command = $description\n  description = $command\nbuild out: cc\n",
            "b.ninja",
        )
        .unwrap_err();
        assert_eq!(
            error.message,
            "cycle in rule variables: command -> description -> command"
        );
    }

    #[test]
    fn rejects_out_of_order_dependency_separators() {
        for statement in [
            "build out: phony || ordered | implicit\n",
            "build out: phony |@ validate || ordered\n",
            "build out: phony | one | two\n",
        ] {
            assert!(parse_manifest(statement, "b.ninja").is_err(), "{statement}");
        }
    }

    #[test]
    fn edge_bindings_expand_against_the_parent_scope() {
        let manifest = parse_manifest(
            "x = global\nrule echo\n  command = echo $y\nbuild out: echo\n  x = edge\n  y = $x\n",
            "b.ninja",
        )
        .unwrap();
        assert_eq!(manifest.edges[0].bindings["y"], "global");
    }

    #[test]
    fn edge_binding_can_break_but_not_hide_a_rule_cycle() {
        parse_manifest(
            "rule cc\n  command = $description\n  description = $command\nbuild good: cc\n  description = okay\n",
            "b.ninja",
        )
        .unwrap();
        let error = parse_manifest(
            "rule cc\n  command = $description\n  description = $command\nbuild good: cc\n  description = okay\nbuild bad: cc\n",
            "b.ninja",
        )
        .unwrap_err();
        assert!(error.message.contains("command -> description -> command"));
    }

    #[test]
    fn enforces_required_syntax_version() {
        parse_manifest("ninja_required_version = 1.14\n", "b.ninja").unwrap();
        let error = parse_manifest("ninja_required_version = 99.0\n", "b.ninja").unwrap_err();
        assert!(error.message.contains("requires Ninja 99.0"));
    }

    #[test]
    fn required_version_compatibility_uses_major_and_minor_like_ninja() {
        for compatible in ["1.14", "1.14.1", "1.14.99", "1.13", "0.99", "garbage"] {
            assert!(!required_version_incompatible(compatible, "1.14.0"));
        }
        for incompatible in ["1.15", "1.15.0", "2.0"] {
            assert!(required_version_incompatible(incompatible, "1.14.0"));
        }
    }

    #[test]
    fn required_version_warning_and_newline_escape_gate_match_ninja_versions() {
        let manifest = parse_manifest("ninja_required_version = garbage\n", "build.ninja")
            .expect("nonnumeric required versions are compatible");
        assert_eq!(manifest.warnings.len(), 1);

        assert!(
            parse_manifest(
                "ninja_required_version = garbage\nvalue = before$^after\n",
                "build.ninja"
            )
            .unwrap_err()
            .message
            .contains("requires specifying 'ninja_required_version'")
        );
        assert!(
            parse_manifest(
                "ninja_required_version = 1.14.99\nvalue = before$^after\n",
                "build.ninja"
            )
            .is_ok()
        );
    }

    #[test]
    fn upstream_lexer_value_identifier_and_escape_corpus() {
        let manifest = parse_manifest(
            concat!(
                "var = lower\n",
                "VaR = mixed\n",
                "x = braced\n",
                "plain = plain text $var $VaR ${x}\n",
                "escaped = $ $$ab c$: $\n",
                " cde\n",
                "foo.dots = dotted\n",
                "bar = base\n",
                "dot-expansion = $bar.dots ${foo.dots}\n",
            ),
            "build.ninja",
        )
        .unwrap();
        assert_eq!(manifest.variables["plain"], "plain text lower mixed braced");
        assert_eq!(manifest.variables["escaped"], " $ab c: cde");
        assert_eq!(manifest.variables["dot-expansion"], "base.dots dotted");

        let comment_eof = parse_manifest("# comment", "build.ninja").unwrap_err();
        assert_eq!(comment_eof.message, "unexpected EOF");
    }

    #[test]
    fn upstream_edit_distance_corpus() {
        assert_eq!(edit_distance(b"", b"ninja", usize::MAX), 5);
        assert_eq!(edit_distance(b"ninja", b"", usize::MAX), 5);
        assert_eq!(edit_distance(b"", b"", usize::MAX), 0);
        for maximum in 1..7 {
            assert_eq!(
                edit_distance(b"abcdefghijklmnop", b"ponmlkjihgfedcba", maximum),
                maximum + 1
            );
        }
        assert_eq!(edit_distance(b"ninja", b"njnja", usize::MAX), 1);
        assert_eq!(edit_distance(b"njnja", b"ninja", usize::MAX), 1);
        assert_eq!(
            edit_distance_with_replacements(b"ninja", b"njnja", false, usize::MAX),
            2
        );
        assert_eq!(
            edit_distance_with_replacements(b"njnja", b"ninja", false, usize::MAX),
            2
        );
        assert_eq!(
            edit_distance(b"browser_tests", b"browser_tests", usize::MAX),
            0
        );
        assert_eq!(
            edit_distance(b"browser_test", b"browser_tests", usize::MAX),
            1
        );
        assert_eq!(
            edit_distance(b"browser_tests", b"browser_test", usize::MAX),
            1
        );
    }
}
