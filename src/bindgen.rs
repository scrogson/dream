//! Generate .dreamt type stubs from Erlang source files.
//!
//! Parses Erlang -spec declarations and converts them to Dream extern mod syntax.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

/// Main entry point for the bindgen command.
pub fn cmd_bindgen(files: &[std::path::PathBuf], output: Option<&Path>, _module: Option<&str>) -> ExitCode {
    let mut all_modules: HashMap<String, Vec<ErlangSpec>> = HashMap::new();

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

        // Parse all -spec declarations
        let specs = parse_specs(&source);

        all_modules
            .entry(module_name)
            .or_default()
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

/// An Erlang function spec.
#[derive(Debug, Clone)]
struct ErlangSpec {
    name: String,
    params: Vec<ErlangType>,
    return_type: ErlangType,
}

/// Erlang type representation.
#[derive(Debug, Clone)]
enum ErlangType {
    /// atom(), integer(), etc.
    Named(String),
    /// list(T)
    List(Box<ErlangType>),
    /// {A, B, C}
    Tuple(Vec<ErlangType>),
    /// fun((A, B) -> C)
    Fun { params: Vec<ErlangType>, ret: Box<ErlangType> },
    /// Type variable like T, A, Number - always becomes 'any' in output
    Var,
    /// Union type: A | B
    Union(Vec<ErlangType>),
    /// any() or term()
    Any,
}

/// Extract module name from -module(name). declaration.
fn extract_module_name(source: &str) -> Option<String> {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("-module(") {
            // Extract name between -module( and ).
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

/// Parse all -spec declarations from Erlang source.
fn parse_specs(source: &str) -> Vec<ErlangSpec> {
    let mut specs = Vec::new();
    let mut in_spec = false;
    let mut spec_text = String::new();

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("-spec") {
            in_spec = true;
            spec_text = trimmed.to_string();

            // Check if spec ends on same line
            if spec_text.ends_with('.') {
                if let Some(spec) = parse_single_spec(&spec_text) {
                    specs.push(spec);
                }
                in_spec = false;
                spec_text.clear();
            }
        } else if in_spec {
            spec_text.push(' ');
            spec_text.push_str(trimmed);

            if trimmed.ends_with('.') {
                if let Some(spec) = parse_single_spec(&spec_text) {
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
fn parse_single_spec(spec: &str) -> Option<ErlangSpec> {
    // Format: -spec name(Type1, Type2) -> RetType.
    // Or: -spec name(Type1, Type2) -> RetType when Var :: Type.

    let spec = spec.trim_start_matches("-spec").trim();
    let spec = spec.trim_end_matches('.');

    // Find function name (before the first '(')
    let paren_pos = spec.find('(')?;
    let mut name = spec[..paren_pos].trim().to_string();

    // Remove module prefix if present (e.g., "erlang:abs" -> "abs")
    if let Some(colon_pos) = name.find(':') {
        name = name[colon_pos + 1..].to_string();
    }

    // Skip operator functions with quoted names like '==', '+', etc.
    if name.starts_with('\'') || name.contains('\'') {
        return None;
    }

    // Skip internal/private functions starting with underscore
    if name.starts_with('_') {
        return None;
    }

    // Split on 'when' to separate main spec from type constraints
    let (main_spec, when_clause) = if let Some(when_pos) = spec.find(" when ") {
        (&spec[paren_pos..when_pos], Some(&spec[when_pos + 6..]))
    } else {
        (&spec[paren_pos..], None)
    };

    // Parse type variable constraints (used for reference, types resolved inline)
    let _type_var_map = parse_when_clause(when_clause);

    // Find the -> to split params from return type
    let arrow_pos = main_spec.rfind("->")?;
    let params_str = &main_spec[1..arrow_pos].trim();
    let params_str = params_str.trim_end_matches(')');
    let return_str = main_spec[arrow_pos + 2..].trim();

    // Parse parameters
    let params = parse_type_list(params_str);

    // Parse return type
    let return_type = parse_type(return_str);

    Some(ErlangSpec {
        name,
        params,
        return_type,
    })
}

/// Parse type constraints from 'when' clause.
fn parse_when_clause(when_clause: Option<&str>) -> HashMap<String, ErlangType> {
    let mut map = HashMap::new();

    if let Some(clause) = when_clause {
        // Split on commas (but not inside parens)
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

/// Parse a comma-separated list of types.
fn parse_type_list(s: &str) -> Vec<ErlangType> {
    if s.trim().is_empty() {
        return Vec::new();
    }

    split_top_level(s, ',')
        .into_iter()
        .map(|t| {
            let t = t.trim();
            // Handle inline type annotations like "Name :: Type"
            let t = if let Some(pos) = t.find("::") {
                t[pos + 2..].trim()
            } else {
                t
            };
            parse_type(t)
        })
        .collect()
}

/// Split string on delimiter, respecting parentheses nesting.
fn split_top_level(s: &str, delim: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
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

    // Strip single quotes from atom type names
    let s = s.trim_matches('\'');

    // Handle inline annotations: "Name :: Type" -> just Type
    let s = if let Some(pos) = s.find("::") {
        s[pos + 2..].trim()
    } else {
        s
    };

    // Skip empty or problematic types
    if s.is_empty() || s.contains('?') {
        return ErlangType::Any;
    }

    // Handle union types (A | B)
    let parts = split_top_level(s, '|');
    if parts.len() > 1 {
        return ErlangType::Union(parts.into_iter().map(|p| parse_type(p.trim())).collect());
    }

    // Handle tuple {A, B, C}
    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len()-1];
        let elements = parse_type_list(inner);
        return ErlangType::Tuple(elements);
    }

    // Handle list [T] or list(T)
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len()-1];
        if inner.is_empty() {
            return ErlangType::List(Box::new(ErlangType::Any));
        }
        return ErlangType::List(Box::new(parse_type(inner)));
    }

    // Handle parameterized types: name(args)
    if let Some(paren_pos) = s.find('(') {
        if s.ends_with(')') {
            let name = &s[..paren_pos];
            let args_str = &s[paren_pos + 1..s.len() - 1];

            match name {
                "list" => {
                    if args_str.is_empty() {
                        return ErlangType::List(Box::new(ErlangType::Any));
                    }
                    return ErlangType::List(Box::new(parse_type(args_str)));
                }
                "fun" => {
                    return parse_fun_type(args_str);
                }
                "function" => {
                    // function() is any function
                    return ErlangType::Fun {
                        params: vec![],
                        ret: Box::new(ErlangType::Any),
                    };
                }
                _ => {
                    // Other parameterized types - treat as named
                    return ErlangType::Named(name.to_string());
                }
            }
        }
    }

    // Handle simple types
    match s {
        "any" | "term" | "_" | "dynamic" => ErlangType::Any,
        "atom" | "boolean" | "true" | "false" | "module" => ErlangType::Named("atom".to_string()),
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
        "port" => ErlangType::Named("any".to_string()), // No direct Dream equivalent
        "node" => ErlangType::Named("atom".to_string()),
        "timeout" | "infinity" => ErlangType::Named("int".to_string()),
        "mfa" => ErlangType::Tuple(vec![
            ErlangType::Named("atom".to_string()),
            ErlangType::Named("atom".to_string()),
            ErlangType::Named("int".to_string()),
        ]),
        "iodata" | "iolist" | "maybe_improper_list" | "nonempty_improper_list" |
        "nonempty_maybe_improper_list" => ErlangType::Named("any".to_string()),
        "char" => ErlangType::Named("int".to_string()),
        "byte" => ErlangType::Named("int".to_string()),
        "nil" | "[]" => ErlangType::List(Box::new(ErlangType::Any)),
        "map" => ErlangType::Named("map".to_string()),
        "tuple" => ErlangType::Named("any".to_string()), // Generic tuple
        "no_return" | "none" => ErlangType::Named("any".to_string()),
        "identifier" => ErlangType::Named("any".to_string()), // pid | port | reference
        s if s.starts_with('\'') => {
            // Atom literal like 'ok' or 'error'
            ErlangType::Named("atom".to_string())
        }
        s if s.starts_with("nonempty_list") => {
            ErlangType::List(Box::new(ErlangType::Any))
        }
        s if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) => {
            // Type variable
            ErlangType::Var
        }
        _ => {
            // Unknown - could be a custom type or atom
            if s.contains(':') {
                // Remote type like module:type()
                ErlangType::Any
            } else {
                // Might be a user-defined type, treat as any
                ErlangType::Any
            }
        }
    }
}

/// Parse fun((A, B) -> C) type.
fn parse_fun_type(s: &str) -> ErlangType {
    let s = s.trim();

    // Format: (Args) -> Return or (() -> Return)
    if s.starts_with('(') {
        if let Some(arrow_pos) = s.find("->") {
            // Find matching paren
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

    // Fallback - generic function
    ErlangType::Fun {
        params: vec![],
        ret: Box::new(ErlangType::Any),
    }
}
/// Check if a type name is a known Dream type
fn is_known_dream_type(name: &str) -> bool {
    matches!(name,
        "int" | "float" | "string" | "atom" | "bool" | "pid" | "ref" |
        "binary" | "map" | "any"
    )
}

/// Check if a name conflicts with Dream keywords
fn is_dream_keyword(name: &str) -> bool {
    matches!(name,
        "binary" | "fn" | "let" | "mut" | "if" | "else" | "match" | "struct" |
        "enum" | "mod" | "pub" | "self" | "spawn" | "receive" | "after" |
        "return" | "use" | "as" | "impl" | "trait" | "for" | "when" | "true" |
        "false" | "extern" | "type" | "string" | "int" | "bool" | "float"
    )
}

/// Generate .dreamt output from parsed specs.
fn generate_dreamt(modules: &HashMap<String, Vec<ErlangSpec>>) -> String {
    let mut output = String::new();
    output.push_str("// Generated by: dream bindgen\n");
    output.push_str("// Do not edit manually.\n\n");

    let mut module_names: Vec<_> = modules.keys().collect();
    module_names.sort();

    for module_name in module_names {
        let specs = &modules[module_name];
        if specs.is_empty() {
            continue;
        }

        // Add suffix for modules that conflict with Dream keywords
        let safe_name = if is_dream_keyword(module_name) {
            format!("{}_", module_name)
        } else {
            module_name.clone()
        };

        output.push_str(&format!("// Erlang module: {}\n", module_name));
        output.push_str(&format!("extern mod {} {{\n", safe_name));

        for spec in specs {
            // No generics - FFI can't enforce Erlang type constraints
            let params: Vec<String> = spec
                .params
                .iter()
                .enumerate()
                .map(|(i, ty)| format!("arg{}: {}", i, erlang_type_to_dream(ty)))
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
            // Convert remaining Erlang type names to Dream equivalents
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
                // Custom Erlang types that don't map to Dream - use any
                "array" | "dict" | "queue" | "gb_tree" | "gb_set" | "set" | "ordset" |
                "digraph" | "tid" | "cp" | "mp" | "re_pattern" | "inet:socket" |
                "socket" | "gen_tcp:socket" | "file:fd" | "file:filename" | "file:name" |
                "unicode:chardata" | "uri_string:uri_string" | "indx_pairs" | "array_indx" |
                "iovec" | "ext_iovec" | "time_unit" | "bitstring_list" => "any".to_string(),
                _ => {
                    // If it looks like a custom type, is empty, or is an unknown lowercase name, use any
                    // Dream only has a small set of builtin types, so unknown types should be any
                    if name.is_empty() || name.contains(':') || name.contains('(') || name.ends_with("()") {
                        "any".to_string()
                    } else if is_known_dream_type(name) {
                        name.clone()
                    } else {
                        // Unknown type name - treat as any
                        "any".to_string()
                    }
                }
            }
        }
        ErlangType::Var => {
            // Type variables become 'any' - FFI can't enforce Erlang's type constraints
            "any".to_string()
        }
        ErlangType::Any => "any".to_string(),
        ErlangType::List(inner) => {
            let inner_ty = erlang_type_to_dream(inner);
            // Ensure we never output an empty type
            if inner_ty == "any" || inner_ty.is_empty() {
                "[any]".to_string()
            } else {
                format!("[{}]", inner_ty)
            }
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
            // Dream doesn't have union types yet, use 'any' for now
            // But if it's just T | undefined, use T
            let non_undefined: Vec<_> = types
                .iter()
                .filter(|t| !matches!(t, ErlangType::Named(n) if n == "undefined"))
                .collect();

            if non_undefined.len() == 1 {
                erlang_type_to_dream(non_undefined[0])
            } else {
                "any".to_string()
            }
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
        let spec = parse_single_spec("-spec abs(Number) -> integer() when Number :: integer().").unwrap();
        assert_eq!(spec.name, "abs");
        assert_eq!(spec.params.len(), 1);
    }

    #[test]
    fn test_parse_spec_no_when() {
        let spec = parse_single_spec("-spec self() -> pid().").unwrap();
        assert_eq!(spec.name, "self");
        assert_eq!(spec.params.len(), 0);
    }

    #[test]
    fn test_parse_list_type() {
        let spec = parse_single_spec("-spec reverse(List1) -> List2 when List1 :: [T], List2 :: [T].").unwrap();
        assert_eq!(spec.name, "reverse");
        // Params are type variables which become 'any'
        assert_eq!(spec.params.len(), 1);
    }

    #[test]
    fn test_parse_fun_type() {
        let spec = parse_single_spec("-spec map(Fun, List1) -> List2 when Fun :: fun((A) -> B), List1 :: [A], List2 :: [B].").unwrap();
        assert_eq!(spec.name, "map");
        assert_eq!(spec.params.len(), 2);
    }
}
