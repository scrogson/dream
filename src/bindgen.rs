//! Generate .dreamt type stubs from Erlang source files.
//!
//! Parses Erlang -spec and -type declarations and converts them to Dream extern mod syntax.
//! Detects Result/Option patterns and generates appropriate Dream types.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

/// Main entry point for the bindgen command.
pub fn cmd_bindgen(files: &[std::path::PathBuf], output: Option<&Path>, _module: Option<&str>) -> ExitCode {
    let mut all_modules: HashMap<String, ModuleInfo> = HashMap::new();

    for file in files {
        if !file.exists() {
            eprintln!("Error: file not found: {}", file.display());
            return ExitCode::from(1);
        }

        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {}: {}", file.display(), e);
                return ExitCode::from(1);
            }
        };

        // Get module name from -module() declaration or filename
        let module_name = extract_module_name(&source)
            .unwrap_or_else(|| {
                file.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            });

        // Parse all -type declarations first to build registry
        let type_defs = parse_type_defs(&source);
        let mut registry = TypeRegistry::new();
        for td in type_defs {
            registry.register(td);
        }

        // Parse all -spec declarations with type resolution
        let specs = parse_specs(&source, &registry);

        all_modules
            .entry(module_name)
            .or_insert_with(ModuleInfo::new)
            .specs
            .extend(specs);
    }

    // Generate output
    let output_content = generate_dreamt(&all_modules);

    // Write to file or stdout
    if let Some(output_path) = output {
        match fs::write(output_path, &output_content) {
            Ok(_) => {
                println!("Generated {}", output_path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("Error writing {}: {}", output_path.display(), e);
                ExitCode::from(1)
            }
        }
    } else {
        let _ = std::io::stdout().write_all(output_content.as_bytes());
        ExitCode::SUCCESS
    }
}

/// Module information including specs and type definitions.
struct ModuleInfo {
    specs: Vec<ErlangSpec>,
}

impl ModuleInfo {
    fn new() -> Self {
        Self { specs: Vec::new() }
    }
}

/// An Erlang type definition (-type or -opaque).
#[derive(Debug, Clone)]
struct ErlangTypeDef {
    name: String,
    #[allow(dead_code)]
    params: Vec<String>,
    definition: ErlangType,
}

/// Type registry for resolving type references.
struct TypeRegistry {
    types: HashMap<String, ErlangTypeDef>,
}

impl TypeRegistry {
    fn new() -> Self {
        Self { types: HashMap::new() }
    }

    fn register(&mut self, typedef: ErlangTypeDef) {
        self.types.insert(typedef.name.clone(), typedef);
    }

    fn resolve(&self, name: &str) -> Option<&ErlangType> {
        self.types.get(name).map(|td| &td.definition)
    }
}

/// An Erlang function spec.
#[derive(Debug, Clone)]
struct ErlangSpec {
    name: String,
    params: Vec<(String, ErlangType)>, // (name, type) pairs
    return_type: ErlangType,
}

/// Erlang type representation.
#[derive(Debug, Clone, PartialEq)]
enum ErlangType {
    /// atom(), integer(), etc.
    Named(String),
    /// Atom literal like 'ok' or 'error'
    AtomLiteral(String),
    /// list(T)
    List(Box<ErlangType>),
    /// {A, B, C}
    Tuple(Vec<ErlangType>),
    /// fun((A, B) -> C)
    Fun { params: Vec<ErlangType>, ret: Box<ErlangType> },
    /// Type variable like T, A, Number
    Var(String),
    /// Union type: A | B
    Union(Vec<ErlangType>),
    /// Result<T, E> - detected from {ok, T} | {error, E}
    Result(Box<ErlangType>, Box<ErlangType>),
    /// Option<T> - detected from T | undefined or {ok, T} | undefined
    Option(Box<ErlangType>),
    /// Remote type like module:type()
    Remote(String, String),
    /// any() or term()
    Any,
}

/// Extract module name from -module(name). declaration.
fn extract_module_name(source: &str) -> Option<String> {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("-module(") {
            let start = "-module(".len();
            if let Some(end) = trimmed.find(").") {
                return Some(trimmed[start..end].trim().to_string());
            }
            if let Some(end) = trimmed.find(')') {
                return Some(trimmed[start..end].trim().to_string());
            }
        }
    }
    None
}

/// Parse all -type declarations from Erlang source.
fn parse_type_defs(source: &str) -> Vec<ErlangTypeDef> {
    let mut type_defs = Vec::new();
    let mut in_type = false;
    let mut type_text = String::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // Skip comments
        if trimmed.starts_with('%') {
            continue;
        }

        if trimmed.starts_with("-type") || trimmed.starts_with("-opaque") {
            in_type = true;
            type_text = trimmed.to_string();

            if type_text.ends_with('.') {
                if let Some(td) = parse_single_type_def(&type_text) {
                    type_defs.push(td);
                }
                in_type = false;
                type_text.clear();
            }
        } else if in_type {
            type_text.push(' ');
            type_text.push_str(trimmed);

            if trimmed.ends_with('.') {
                if let Some(td) = parse_single_type_def(&type_text) {
                    type_defs.push(td);
                }
                in_type = false;
                type_text.clear();
            }
        }
    }

    type_defs
}

/// Parse a single -type declaration.
fn parse_single_type_def(text: &str) -> Option<ErlangTypeDef> {
    // Format: -type name() :: definition.
    // Or: -type name(Param) :: definition.
    let text = text.trim_start_matches("-type")
        .trim_start_matches("-opaque")
        .trim();
    let text = text.trim_end_matches('.');

    // Find name and params
    let paren_pos = text.find('(')?;
    let name = text[..paren_pos].trim().to_string();

    // Find :: separator
    let def_pos = text.find("::")?;

    // Parse params between ( and )
    let params_end = text[..def_pos].rfind(')')?;
    let params_str = &text[paren_pos + 1..params_end];
    let params: Vec<String> = if params_str.trim().is_empty() {
        Vec::new()
    } else {
        split_top_level(params_str, ',')
            .into_iter()
            .map(|s| s.trim().to_string())
            .collect()
    };

    // Parse definition
    let def_str = text[def_pos + 2..].trim();
    let definition = parse_type(def_str);

    Some(ErlangTypeDef {
        name,
        params,
        definition,
    })
}

/// Parse all -spec declarations from Erlang source.
fn parse_specs(source: &str, registry: &TypeRegistry) -> Vec<ErlangSpec> {
    let mut specs = Vec::new();
    let mut in_spec = false;
    let mut spec_text = String::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // Skip comments
        if trimmed.starts_with('%') {
            continue;
        }

        if trimmed.starts_with("-spec") {
            in_spec = true;
            spec_text = trimmed.to_string();

            if spec_text.ends_with('.') {
                if let Some(spec) = parse_single_spec(&spec_text, registry) {
                    specs.push(spec);
                }
                in_spec = false;
                spec_text.clear();
            }
        } else if in_spec {
            spec_text.push(' ');
            spec_text.push_str(trimmed);

            if trimmed.ends_with('.') {
                if let Some(spec) = parse_single_spec(&spec_text, registry) {
                    specs.push(spec);
                }
                in_spec = false;
                spec_text.clear();
            }
        }
    }

    specs
}

/// Parse a single -spec declaration.
fn parse_single_spec(spec: &str, registry: &TypeRegistry) -> Option<ErlangSpec> {
    let spec = spec.trim_start_matches("-spec").trim();
    let spec = spec.trim_end_matches('.');

    // Find function name
    let paren_pos = spec.find('(')?;
    let mut name = spec[..paren_pos].trim().to_string();

    // Remove module prefix if present
    if let Some(colon_pos) = name.find(':') {
        name = name[colon_pos + 1..].to_string();
    }

    // Skip operators and internal functions
    if name.starts_with('\'') || name.contains('\'') || name.starts_with('_') {
        return None;
    }

    // Split on 'when' to separate main spec from type constraints
    let (main_spec, when_clause) = if let Some(when_pos) = spec.find(" when ") {
        (&spec[paren_pos..when_pos], Some(&spec[when_pos + 6..]))
    } else {
        (&spec[paren_pos..], None)
    };

    // Parse type variable constraints
    let type_var_map = parse_when_clause(when_clause);

    // Find -> to split params from return type
    let arrow_pos = main_spec.rfind("->")?;
    let params_str = &main_spec[1..arrow_pos].trim();
    let params_str = params_str.trim_end_matches(')');
    let return_str = main_spec[arrow_pos + 2..].trim();

    // Parse parameters with names
    let params = parse_param_list(params_str, &type_var_map, registry);

    // Parse return type and apply pattern detection
    let mut return_type = parse_type(return_str);
    return_type = resolve_type_vars(&return_type, &type_var_map);
    return_type = resolve_type_refs(&return_type, registry);
    return_type = detect_result_option_pattern(return_type);

    Some(ErlangSpec {
        name,
        params,
        return_type,
    })
}

/// Parse parameter list with names.
fn parse_param_list(s: &str, type_var_map: &HashMap<String, ErlangType>, registry: &TypeRegistry) -> Vec<(String, ErlangType)> {
    if s.trim().is_empty() {
        return Vec::new();
    }

    split_top_level(s, ',')
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let t = t.trim();
            // Check for "Name :: Type" annotation
            if let Some(pos) = t.find("::") {
                let param_name = t[..pos].trim();
                // Sanitize param name (removes " | undefined" etc.)
                let param_name = sanitize_param_name(param_name);
                let type_str = t[pos + 2..].trim();
                let mut ty = parse_type(type_str);
                ty = resolve_type_vars(&ty, type_var_map);
                ty = resolve_type_refs(&ty, registry);
                (param_name, ty)
            } else {
                // Just a type variable or type name
                let param_name = if t.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    // Type variable - use lowercase version as name
                    sanitize_param_name(&t.to_lowercase())
                } else {
                    format!("arg{}", i)
                };
                let mut ty = parse_type(t);
                ty = resolve_type_vars(&ty, type_var_map);
                ty = resolve_type_refs(&ty, registry);
                (param_name, ty)
            }
        })
        .collect()
}

/// Sanitize a parameter name to be a valid Dream identifier.
/// Handles Erlang union types in param names like "Req | undefined".
fn sanitize_param_name(name: &str) -> String {
    // Strip union type suffixes (e.g., "Req | undefined" -> "Req")
    let name = if let Some(pos) = name.find('|') {
        name[..pos].trim()
    } else {
        name
    };

    // Convert to lowercase for Dream convention
    let name = name.to_lowercase();

    // Replace any invalid characters with underscores
    let name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect();

    // Ensure it doesn't start with a digit
    if name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        format!("arg_{}", name)
    } else {
        name
    }
}

/// Parse type constraints from 'when' clause.
fn parse_when_clause(when_clause: Option<&str>) -> HashMap<String, ErlangType> {
    let mut map = HashMap::new();

    if let Some(clause) = when_clause {
        for constraint in split_top_level(clause, ',') {
            let constraint = constraint.trim();
            if let Some(pos) = constraint.find("::") {
                let var = constraint[..pos].trim().to_string();
                let ty = parse_type(constraint[pos + 2..].trim());
                map.insert(var, ty);
            }
        }
    }

    map
}

/// Resolve type variable references using the when clause map.
fn resolve_type_vars(ty: &ErlangType, type_var_map: &HashMap<String, ErlangType>) -> ErlangType {
    let mut visited = std::collections::HashSet::new();
    resolve_type_vars_inner(ty, type_var_map, &mut visited)
}

fn resolve_type_vars_inner(
    ty: &ErlangType,
    type_var_map: &HashMap<String, ErlangType>,
    visited: &mut std::collections::HashSet<String>,
) -> ErlangType {
    match ty {
        ErlangType::Var(name) => {
            // Prevent infinite recursion on recursive type definitions like DeepList :: [term() | DeepList]
            if visited.contains(name) {
                return ErlangType::Any;
            }
            if let Some(resolved) = type_var_map.get(name) {
                visited.insert(name.clone());
                let result = resolve_type_vars_inner(resolved, type_var_map, visited);
                visited.remove(name);
                result
            } else {
                ErlangType::Any
            }
        }
        ErlangType::List(inner) => {
            ErlangType::List(Box::new(resolve_type_vars_inner(inner, type_var_map, visited)))
        }
        ErlangType::Tuple(elements) => {
            ErlangType::Tuple(elements.iter().map(|e| resolve_type_vars_inner(e, type_var_map, visited)).collect())
        }
        ErlangType::Fun { params, ret } => {
            ErlangType::Fun {
                params: params.iter().map(|p| resolve_type_vars_inner(p, type_var_map, visited)).collect(),
                ret: Box::new(resolve_type_vars_inner(ret, type_var_map, visited)),
            }
        }
        ErlangType::Union(types) => {
            ErlangType::Union(types.iter().map(|t| resolve_type_vars_inner(t, type_var_map, visited)).collect())
        }
        _ => ty.clone(),
    }
}

/// Resolve type references using the type registry.
fn resolve_type_refs(ty: &ErlangType, registry: &TypeRegistry) -> ErlangType {
    let mut visited = std::collections::HashSet::new();
    resolve_type_refs_inner(ty, registry, &mut visited)
}

fn resolve_type_refs_inner(
    ty: &ErlangType,
    registry: &TypeRegistry,
    visited: &mut std::collections::HashSet<String>,
) -> ErlangType {
    match ty {
        ErlangType::Named(name) => {
            // Prevent infinite recursion on cyclic type definitions
            if visited.contains(name) {
                return ty.clone();
            }
            if let Some(resolved) = registry.resolve(name) {
                visited.insert(name.clone());
                let result = resolve_type_refs_inner(resolved, registry, visited);
                visited.remove(name);
                result
            } else {
                ty.clone()
            }
        }
        ErlangType::Remote(module, name) => {
            resolve_remote_type(module, name)
        }
        ErlangType::List(inner) => {
            ErlangType::List(Box::new(resolve_type_refs_inner(inner, registry, visited)))
        }
        ErlangType::Tuple(elements) => {
            ErlangType::Tuple(elements.iter().map(|e| resolve_type_refs_inner(e, registry, visited)).collect())
        }
        ErlangType::Fun { params, ret } => {
            ErlangType::Fun {
                params: params.iter().map(|p| resolve_type_refs_inner(p, registry, visited)).collect(),
                ret: Box::new(resolve_type_refs_inner(ret, registry, visited)),
            }
        }
        ErlangType::Union(types) => {
            ErlangType::Union(types.iter().map(|t| resolve_type_refs_inner(t, registry, visited)).collect())
        }
        _ => ty.clone(),
    }
}

/// Resolve known remote types.
fn resolve_remote_type(module: &str, name: &str) -> ErlangType {
    match (module, name) {
        // calendar module
        ("calendar", "datetime") => ErlangType::Tuple(vec![
            ErlangType::Tuple(vec![
                ErlangType::Named("int".into()),
                ErlangType::Named("int".into()),
                ErlangType::Named("int".into()),
            ]),
            ErlangType::Tuple(vec![
                ErlangType::Named("int".into()),
                ErlangType::Named("int".into()),
                ErlangType::Named("int".into()),
            ]),
        ]),
        ("calendar", "date") => ErlangType::Tuple(vec![
            ErlangType::Named("int".into()),
            ErlangType::Named("int".into()),
            ErlangType::Named("int".into()),
        ]),
        ("calendar", "time") => ErlangType::Tuple(vec![
            ErlangType::Named("int".into()),
            ErlangType::Named("int".into()),
            ErlangType::Named("int".into()),
        ]),

        // file module
        ("file", "filename") | ("file", "name") | ("file", "name_all") | ("file", "filename_all") => {
            ErlangType::Named("string".into())
        }
        ("file", "posix") => ErlangType::Named("atom".into()),
        ("file", "io_device") => ErlangType::Named("any".into()),

        // io module
        ("io", "device") => ErlangType::Named("any".into()),

        // unicode
        ("unicode", "chardata") | ("unicode", "charlist") => ErlangType::Any,

        // erlang
        ("erlang", "timestamp") => ErlangType::Tuple(vec![
            ErlangType::Named("int".into()),
            ErlangType::Named("int".into()),
            ErlangType::Named("int".into()),
        ]),

        _ => ErlangType::Any,
    }
}

/// Detect Result/Option patterns in union types.
fn detect_result_option_pattern(ty: ErlangType) -> ErlangType {
    match ty {
        ErlangType::Union(types) => {
            // Try to detect true | false -> boolean
            if detect_boolean_pattern(&types) {
                return ErlangType::Named("boolean".to_string());
            }

            // Try to detect {ok, T} | {error, E} -> Result<T, E>
            if let Some((ok_type, err_type)) = detect_ok_error_pattern(&types) {
                return ErlangType::Result(Box::new(ok_type), Box::new(err_type));
            }

            // Try to detect ok | {error, E} -> Result<(), E>
            if let Some(err_type) = detect_ok_atom_error_pattern(&types) {
                return ErlangType::Result(
                    Box::new(ErlangType::Tuple(vec![])),
                    Box::new(err_type),
                );
            }

            // Try to detect T | undefined -> Option<T>
            if let Some(inner_type) = detect_undefined_pattern(&types) {
                return ErlangType::Option(Box::new(inner_type));
            }

            // No pattern detected, keep as union
            ErlangType::Union(types)
        }
        _ => ty,
    }
}

/// Detect true | false pattern (boolean type).
fn detect_boolean_pattern(types: &[ErlangType]) -> bool {
    if types.len() != 2 {
        return false;
    }

    let mut has_true = false;
    let mut has_false = false;

    for ty in types {
        match ty {
            ErlangType::AtomLiteral(name) if name == "true" => has_true = true,
            ErlangType::AtomLiteral(name) if name == "false" => has_false = true,
            _ => {}
        }
    }

    has_true && has_false
}

/// Detect {ok, T} | {error, E} pattern.
fn detect_ok_error_pattern(types: &[ErlangType]) -> Option<(ErlangType, ErlangType)> {
    if types.len() != 2 {
        return None;
    }

    let mut ok_type = None;
    let mut err_type = None;

    for ty in types {
        if let ErlangType::Tuple(elements) = ty {
            if elements.len() == 2 {
                if is_ok_atom(&elements[0]) {
                    ok_type = Some(elements[1].clone());
                } else if is_error_atom(&elements[0]) {
                    err_type = Some(elements[1].clone());
                }
            }
        }
    }

    match (ok_type, err_type) {
        (Some(ok), Some(err)) => Some((ok, err)),
        _ => None,
    }
}

/// Detect ok | {error, E} pattern.
fn detect_ok_atom_error_pattern(types: &[ErlangType]) -> Option<ErlangType> {
    if types.len() != 2 {
        return None;
    }

    let mut has_ok_atom = false;
    let mut err_type = None;

    for ty in types {
        match ty {
            ErlangType::AtomLiteral(name) | ErlangType::Named(name) if name == "ok" || name == "atom" => {
                // Bare 'ok' atom
                has_ok_atom = true;
            }
            ErlangType::Tuple(elements) if elements.len() == 2 && is_error_atom(&elements[0]) => {
                err_type = Some(elements[1].clone());
            }
            _ => {}
        }
    }

    if has_ok_atom && err_type.is_some() {
        err_type
    } else {
        None
    }
}

/// Detect T | undefined pattern.
fn detect_undefined_pattern(types: &[ErlangType]) -> Option<ErlangType> {
    if types.len() != 2 {
        return None;
    }

    let mut undefined_found = false;
    let mut other_type = None;

    for ty in types {
        match ty {
            ErlangType::AtomLiteral(name) | ErlangType::Named(name)
                if name == "undefined" || name == "nil" => {
                undefined_found = true;
            }
            _ => {
                other_type = Some(ty.clone());
            }
        }
    }

    if undefined_found {
        other_type
    } else {
        None
    }
}

fn is_ok_atom(ty: &ErlangType) -> bool {
    matches!(ty, ErlangType::AtomLiteral(n) | ErlangType::Named(n) if n == "ok" || n == "atom")
}

fn is_error_atom(ty: &ErlangType) -> bool {
    matches!(ty, ErlangType::AtomLiteral(n) | ErlangType::Named(n) if n == "error" || n == "atom")
}

/// Split string on delimiter, respecting parentheses nesting.
fn split_top_level(s: &str, delim: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth: usize = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            c if c == delim && depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }

    if start < s.len() {
        result.push(&s[start..]);
    }

    result
}

/// Parse a single Erlang type.
fn parse_type(s: &str) -> ErlangType {
    let s = s.trim();

    // Empty type
    if s.is_empty() {
        return ErlangType::Any;
    }

    // Check for atom literals BEFORE stripping quotes
    if s.starts_with('\'') && s.ends_with('\'') && s.len() > 2 {
        let atom_name = &s[1..s.len()-1];
        return ErlangType::AtomLiteral(atom_name.to_string());
    }

    // Handle inline annotations: "Name :: Type"
    if let Some(pos) = s.find("::") {
        return parse_type(s[pos + 2..].trim());
    }

    // Skip problematic types
    if s.contains('?') || s.starts_with('#') {
        return ErlangType::Any;
    }

    // Handle union types (A | B)
    let parts = split_top_level(s, '|');
    if parts.len() > 1 {
        let types: Vec<ErlangType> = parts.iter().map(|p| parse_type(p.trim())).collect();
        return detect_result_option_pattern(ErlangType::Union(types));
    }

    // Handle tuple {A, B, C}
    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len()-1];
        let elements = parse_type_list(inner);
        return ErlangType::Tuple(elements);
    }

    // Handle list [T]
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len()-1];
        if inner.is_empty() {
            return ErlangType::List(Box::new(ErlangType::Any));
        }
        return ErlangType::List(Box::new(parse_type(inner)));
    }

    // Handle remote types: module:type() or module:type
    if s.contains(':') && !s.starts_with(':') {
        if let Some(colon_pos) = s.find(':') {
            let module = &s[..colon_pos];
            let rest = &s[colon_pos + 1..];
            let type_name = if let Some(paren_pos) = rest.find('(') {
                &rest[..paren_pos]
            } else {
                rest
            };
            return ErlangType::Remote(module.to_string(), type_name.to_string());
        }
    }

    // Handle parameterized types: name(args)
    if let Some(paren_pos) = s.find('(') {
        if s.ends_with(')') {
            let name = &s[..paren_pos];
            let args_str = &s[paren_pos + 1..s.len() - 1];

            match name {
                "list" | "nonempty_list" => {
                    if args_str.is_empty() {
                        return ErlangType::List(Box::new(ErlangType::Any));
                    }
                    return ErlangType::List(Box::new(parse_type(args_str)));
                }
                "fun" => {
                    return parse_fun_type(args_str);
                }
                "function" => {
                    return ErlangType::Fun {
                        params: vec![],
                        ret: Box::new(ErlangType::Any),
                    };
                }
                "maybe_improper_list" | "nonempty_maybe_improper_list" | "nonempty_improper_list" => {
                    return ErlangType::List(Box::new(ErlangType::Any));
                }
                _ => {
                    // Other parameterized types
                    return ErlangType::Named(name.to_string());
                }
            }
        }
    }

    // Handle simple types
    match s {
        "any" | "term" | "_" | "dynamic" => ErlangType::Any,
        "atom" | "module" => ErlangType::Named("atom".to_string()),
        "boolean" => ErlangType::Named("boolean".to_string()),
        "true" => ErlangType::AtomLiteral("true".to_string()),
        "false" => ErlangType::AtomLiteral("false".to_string()),
        "ok" => ErlangType::AtomLiteral("ok".to_string()),
        "error" => ErlangType::AtomLiteral("error".to_string()),
        "undefined" => ErlangType::AtomLiteral("undefined".to_string()),
        "nil" => ErlangType::AtomLiteral("nil".to_string()),
        "integer" | "non_neg_integer" | "pos_integer" | "neg_integer" | "number" | "arity" => {
            ErlangType::Named("int".to_string())
        }
        "float" => ErlangType::Named("float".to_string()),
        "binary" | "bitstring" | "nonempty_binary" | "nonempty_bitstring" => {
            ErlangType::Named("binary".to_string())
        }
        "string" | "nonempty_string" => ErlangType::Named("string".to_string()),
        "pid" => ErlangType::Named("pid".to_string()),
        "reference" | "ref" => ErlangType::Named("ref".to_string()),
        "port" => ErlangType::Any,
        "node" => ErlangType::Named("atom".to_string()),
        "timeout" | "infinity" => ErlangType::Named("int".to_string()),
        "mfa" => ErlangType::Tuple(vec![
            ErlangType::Named("atom".to_string()),
            ErlangType::Named("atom".to_string()),
            ErlangType::Named("int".to_string()),
        ]),
        "iodata" | "iolist" => ErlangType::Any,
        "char" | "byte" => ErlangType::Named("int".to_string()),
        "[]" => ErlangType::List(Box::new(ErlangType::Any)),
        "map" => ErlangType::Named("map".to_string()),
        "tuple" => ErlangType::Any,
        "no_return" | "none" => ErlangType::Any,
        "identifier" => ErlangType::Any,
        s if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) => {
            ErlangType::Var(s.to_string())
        }
        _ => {
            // Unknown type name - might be a user-defined type
            ErlangType::Named(s.to_string())
        }
    }
}

/// Parse a comma-separated list of types.
fn parse_type_list(s: &str) -> Vec<ErlangType> {
    if s.trim().is_empty() {
        return Vec::new();
    }

    split_top_level(s, ',')
        .into_iter()
        .map(|t| parse_type(t.trim()))
        .collect()
}

/// Parse fun((A, B) -> C) type.
fn parse_fun_type(s: &str) -> ErlangType {
    let s = s.trim();

    if s.starts_with('(') {
        if let Some(arrow_pos) = s.find("->") {
            let mut depth = 0;
            let mut params_end = 0;
            for (i, c) in s.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            params_end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            let params_str = &s[1..params_end];
            let ret_str = &s[arrow_pos + 2..];

            let params = parse_type_list(params_str);
            let ret = parse_type(ret_str);

            return ErlangType::Fun {
                params,
                ret: Box::new(ret),
            };
        }
    }

    ErlangType::Fun {
        params: vec![],
        ret: Box::new(ErlangType::Any),
    }
}

/// Check if a type name is a known Dream type.
fn is_known_dream_type(name: &str) -> bool {
    matches!(name,
        "int" | "float" | "string" | "atom" | "bool" | "pid" | "ref" |
        "binary" | "map" | "any"
    )
}

/// Check if a name conflicts with Dream keywords.
fn is_dream_keyword(name: &str) -> bool {
    matches!(name,
        "binary" | "fn" | "let" | "mut" | "if" | "else" | "match" | "struct" |
        "enum" | "mod" | "pub" | "self" | "spawn" | "receive" | "after" |
        "return" | "use" | "as" | "impl" | "trait" | "for" | "when" | "true" |
        "false" | "extern" | "type" | "string" | "int" | "bool" | "float" |
        "integer" | "atom" | "pid" | "ref" | "map" | "list" | "tuple" | "bytes"
    )
}

/// Sanitize a module name to be a valid Dream identifier.
/// Handles Elixir modules (Elixir.Foo.Bar -> foo_bar) and other special cases.
fn sanitize_module_name(name: &str) -> String {
    // Handle Elixir modules: Elixir.Foo.Bar -> foo_bar
    if name.starts_with("Elixir.") {
        let without_prefix = &name[7..]; // Remove "Elixir."
        return without_prefix
            .split('.')
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>()
            .join("_");
    }

    // Handle quoted atom names: 'Module.Name' -> module_name
    let name = name.trim_matches('\'');

    // Handle other dotted names
    if name.contains('.') {
        let sanitized = name
            .split('.')
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>()
            .join("_");
        return sanitize_identifier(&sanitized);
    }

    // Regular name - just ensure it's a valid identifier
    sanitize_identifier(name)
}

/// Ensure a string is a valid Dream identifier.
fn sanitize_identifier(name: &str) -> String {
    let mut result = String::new();

    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_alphanumeric() || c == '_' {
            // First char can't be a digit
            if i == 0 && c.is_ascii_digit() {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else if c == '-' || c == '.' {
            result.push('_');
        }
        // Skip other invalid characters
    }

    // Handle empty result
    if result.is_empty() {
        return "module_".to_string();
    }

    // Handle keywords by appending underscore
    if is_dream_keyword(&result) {
        result.push('_');
    }

    result
}

/// Generate .dreamt output from parsed module info.
fn generate_dreamt(modules: &HashMap<String, ModuleInfo>) -> String {
    let mut output = String::new();
    output.push_str("// Generated by: dream bindgen\n");
    output.push_str("// Do not edit manually.\n\n");

    let mut module_names: Vec<_> = modules.keys().collect();
    module_names.sort();

    for module_name in module_names {
        let info = &modules[module_name];
        if info.specs.is_empty() {
            continue;
        }

        // Sanitize the module name to be a valid Dream identifier
        let safe_name = sanitize_module_name(module_name);

        output.push_str(&format!("// Erlang module: {}\n", module_name));

        // Generate #[name = "..."] attribute if the sanitized name differs from original
        if safe_name != *module_name {
            output.push_str(&format!("#[name = \"{}\"]\n", module_name));
        }

        output.push_str(&format!("extern mod {} {{\n", safe_name));

        for spec in &info.specs {
            let params: Vec<String> = spec
                .params
                .iter()
                .map(|(name, ty)| {
                    let safe_name = if is_dream_keyword(name) {
                        format!("{}_", name)
                    } else {
                        name.clone()
                    };
                    format!("{}: {}", safe_name, erlang_type_to_dream(ty))
                })
                .collect();

            let ret = erlang_type_to_dream(&spec.return_type);

            output.push_str(&format!(
                "    fn {}({}) -> {};\n",
                spec.name,
                params.join(", "),
                ret
            ));
        }

        output.push_str("}\n\n");
    }

    output
}

/// Convert Erlang type to Dream type syntax.
fn erlang_type_to_dream(ty: &ErlangType) -> String {
    match ty {
        ErlangType::Named(name) => {
            match name.as_str() {
                "integer" | "non_neg_integer" | "pos_integer" | "neg_integer" | "number" => "int".to_string(),
                "boolean" => "bool".to_string(),
                "atom" => "atom".to_string(),
                "binary" | "bitstring" => "binary".to_string(),
                "float" => "float".to_string(),
                "string" => "string".to_string(),
                "pid" => "pid".to_string(),
                "reference" | "ref" => "ref".to_string(),
                "map" => "map".to_string(),
                "term" => "any".to_string(),
                "byte" | "char" | "arity" => "int".to_string(),
                "no_return" | "none" => "any".to_string(),
                _ => {
                    if name.is_empty() || name.contains(':') || name.contains('(') {
                        "any".to_string()
                    } else if is_known_dream_type(name) {
                        name.clone()
                    } else {
                        "any".to_string()
                    }
                }
            }
        }
        ErlangType::AtomLiteral(_) => "atom".to_string(),
        ErlangType::Var(_) => "any".to_string(),
        ErlangType::Any => "any".to_string(),
        ErlangType::Remote(_, _) => "any".to_string(),
        ErlangType::List(inner) => {
            let inner_ty = erlang_type_to_dream(inner);
            format!("[{}]", inner_ty)
        }
        ErlangType::Tuple(elements) => {
            if elements.is_empty() {
                "()".to_string()
            } else {
                let parts: Vec<String> = elements.iter().map(erlang_type_to_dream).collect();
                format!("({})", parts.join(", "))
            }
        }
        ErlangType::Fun { params, ret } => {
            let param_types: Vec<String> = params.iter().map(erlang_type_to_dream).collect();
            format!("fn({}) -> {}", param_types.join(", "), erlang_type_to_dream(ret))
        }
        ErlangType::Union(types) => {
            // First try to detect Result/Option pattern
            let detected = detect_result_option_pattern(ErlangType::Union(types.clone()));
            if !matches!(detected, ErlangType::Union(_)) {
                return erlang_type_to_dream(&detected);
            }

            // Filter out undefined for Option-like types
            let non_undefined: Vec<_> = types
                .iter()
                .filter(|t| !matches!(t,
                    ErlangType::AtomLiteral(n) | ErlangType::Named(n)
                    if n == "undefined" || n == "nil"
                ))
                .collect();

            if non_undefined.len() == 1 {
                erlang_type_to_dream(non_undefined[0])
            } else {
                "any".to_string()
            }
        }
        ErlangType::Result(ok_ty, err_ty) => {
            format!("Result<{}, {}>", erlang_type_to_dream(ok_ty), erlang_type_to_dream(err_ty))
        }
        ErlangType::Option(inner) => {
            format!("Option<{}>", erlang_type_to_dream(inner))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_module_name() {
        assert_eq!(
            extract_module_name("-module(lists)."),
            Some("lists".to_string())
        );
        assert_eq!(
            extract_module_name("-module(erlang).\n-export([abs/1])."),
            Some("erlang".to_string())
        );
    }

    #[test]
    fn test_parse_simple_spec() {
        let registry = TypeRegistry::new();
        let spec = parse_single_spec("-spec abs(Number) -> integer() when Number :: integer().", &registry).unwrap();
        assert_eq!(spec.name, "abs");
        assert_eq!(spec.params.len(), 1);
    }

    #[test]
    fn test_parse_spec_no_when() {
        let registry = TypeRegistry::new();
        let spec = parse_single_spec("-spec self() -> pid().", &registry).unwrap();
        assert_eq!(spec.name, "self");
        assert_eq!(spec.params.len(), 0);
    }

    #[test]
    fn test_parse_list_type() {
        let registry = TypeRegistry::new();
        let spec = parse_single_spec("-spec reverse(List1) -> List2 when List1 :: [T], List2 :: [T].", &registry).unwrap();
        assert_eq!(spec.name, "reverse");
        assert_eq!(spec.params.len(), 1);
    }

    #[test]
    fn test_parse_fun_type() {
        let registry = TypeRegistry::new();
        let spec = parse_single_spec("-spec map(Fun, List1) -> List2 when Fun :: fun((A) -> B), List1 :: [A], List2 :: [B].", &registry).unwrap();
        assert_eq!(spec.name, "map");
        assert_eq!(spec.params.len(), 2);
    }

    #[test]
    fn test_parse_type_def() {
        let td = parse_single_type_def("-type filename() :: string().").unwrap();
        assert_eq!(td.name, "filename");
        assert!(td.params.is_empty());
        assert!(matches!(td.definition, ErlangType::Named(n) if n == "string"));
    }

    #[test]
    fn test_parse_type_def_with_params() {
        let td = parse_single_type_def("-type container(T) :: {ok, T} | {error, term()}.").unwrap();
        assert_eq!(td.name, "container");
        assert_eq!(td.params, vec!["T"]);
    }

    #[test]
    fn test_result_pattern_detection() {
        let ty = parse_type("{ok, binary()} | {error, atom()}");
        let detected = detect_result_option_pattern(ty);
        assert!(matches!(detected, ErlangType::Result(_, _)));
    }

    #[test]
    fn test_unit_result_pattern() {
        let ty = parse_type("ok | {error, Reason}");
        let detected = detect_result_option_pattern(ty);
        assert!(matches!(detected, ErlangType::Result(ok_ty, _) if matches!(*ok_ty, ErlangType::Tuple(ref v) if v.is_empty())));
    }

    #[test]
    fn test_option_pattern_detection() {
        let ty = parse_type("{ok, Value} | undefined");
        let detected = detect_result_option_pattern(ty);
        assert!(matches!(detected, ErlangType::Option(_)));
    }

    #[test]
    fn test_remote_type_parsing() {
        let ty = parse_type("calendar:datetime()");
        assert!(matches!(ty, ErlangType::Remote(m, n) if m == "calendar" && n == "datetime"));
    }

    #[test]
    fn test_result_type_to_dream() {
        let ty = ErlangType::Result(
            Box::new(ErlangType::Named("binary".into())),
            Box::new(ErlangType::Named("atom".into())),
        );
        assert_eq!(erlang_type_to_dream(&ty), "Result<binary, atom>");
    }

    #[test]
    fn test_option_type_to_dream() {
        let ty = ErlangType::Option(Box::new(ErlangType::Named("string".into())));
        assert_eq!(erlang_type_to_dream(&ty), "Option<string>");
    }

    #[test]
    fn test_sanitize_elixir_module_name() {
        // Elixir.Jason -> jason
        assert_eq!(sanitize_module_name("Elixir.Jason"), "jason");
        // Elixir.Jason.Encoder -> jason_encoder
        assert_eq!(sanitize_module_name("Elixir.Jason.Encoder"), "jason_encoder");
        // Elixir.Phoenix.Controller -> phoenix_controller
        assert_eq!(sanitize_module_name("Elixir.Phoenix.Controller"), "phoenix_controller");
    }

    #[test]
    fn test_sanitize_regular_module_name() {
        // Regular Erlang modules unchanged
        assert_eq!(sanitize_module_name("lists"), "lists");
        assert_eq!(sanitize_module_name("erlang"), "erlang");
        assert_eq!(sanitize_module_name("file"), "file");
    }

    #[test]
    fn test_sanitize_keyword_module_name() {
        // Keywords get underscore suffix
        assert_eq!(sanitize_module_name("string"), "string_");
        assert_eq!(sanitize_module_name("binary"), "binary_");
    }

    #[test]
    fn test_sanitize_dotted_module_name() {
        // Other dotted names
        assert_eq!(sanitize_module_name("foo.bar"), "foo_bar");
        assert_eq!(sanitize_module_name("cowboy_req"), "cowboy_req");
    }

    #[test]
    fn test_generate_name_attribute() {
        // Test that #[name = "..."] is generated when names differ
        let mut modules = HashMap::new();
        modules.insert("Elixir.Jason".to_string(), ModuleInfo {
            specs: vec![ErlangSpec {
                name: "encode".to_string(),
                params: vec![("input".to_string(), ErlangType::Any)],
                return_type: ErlangType::Any,
            }],
        });

        let output = generate_dreamt(&modules);

        // Should contain #[name = "Elixir.Jason"]
        assert!(output.contains("#[name = \"Elixir.Jason\"]"),
            "Expected #[name = \"Elixir.Jason\"], got:\n{}", output);
        // Should use sanitized name 'jason'
        assert!(output.contains("extern mod jason"),
            "Expected 'extern mod jason', got:\n{}", output);
    }

    #[test]
    fn test_no_name_attribute_for_simple_modules() {
        // Test that #[name = "..."] is NOT generated when names match
        let mut modules = HashMap::new();
        modules.insert("lists".to_string(), ModuleInfo {
            specs: vec![ErlangSpec {
                name: "reverse".to_string(),
                params: vec![("list".to_string(), ErlangType::List(Box::new(ErlangType::Any)))],
                return_type: ErlangType::List(Box::new(ErlangType::Any)),
            }],
        });

        let output = generate_dreamt(&modules);

        // Should NOT contain #[name = "lists"]
        assert!(!output.contains("#[name ="),
            "Should not have #[name = ...] for simple module, got:\n{}", output);
        // Should use simple name
        assert!(output.contains("extern mod lists"),
            "Expected 'extern mod lists', got:\n{}", output);
    }
}
