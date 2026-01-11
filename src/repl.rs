//! Dream interactive shell (REPL)
//!
//! Provides an interactive environment for evaluating Dream expressions
//! using the BEAM runtime with a persistent process for fast evaluation.
//! Uses runtime introspection for module discovery and method dispatch.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, ExitCode, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};

use dream::compiler::{BinOp, Expr, Parser, StringPart};

/// Counter for generating unique module names
static EVAL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Result marker for protocol
const RESULT_MARKER: &str = "\x00DREAM_RESULT\x00";

/// Erlang eval server with introspection support
/// Protocol:
///   eval:<filename> - evaluate a Core Erlang file
///   introspect:list_modules - list all dream:: modules
///   introspect:exports:<module> - get exports for a module
const EVAL_SERVER: &str = r#"
Loop = fun Loop() ->
    case io:get_line("") of
        eof -> ok;
        {error, _} -> ok;
        Line ->
            Cmd = string:trim(Line),
            case Cmd of
                "" -> Loop();
                "introspect:list_modules" ->
                    Paths = code:get_path(),
                    Mods = lists:usort(lists:flatten([
                        [begin
                            Base = filename:basename(F, ".beam"),
                            list_to_atom(Base)
                        end || F <- filelib:wildcard("dream::*.beam", Dir)]
                    || Dir <- Paths])),
                    io:format("~s~nok:~w~n", [<<0, "DREAM_RESULT", 0>>, Mods]),
                    Loop();
                _ ->
                    case string:prefix(Cmd, "introspect:exports:") of
                        nomatch ->
                            case string:prefix(Cmd, "eval:") of
                                nomatch ->
                                    io:format("~s~nerr:unknown_command~n", [<<0, "DREAM_RESULT", 0>>]),
                                    Loop();
                                Filename ->
                                    Result = try
                                        ModName = list_to_atom(filename:basename(Filename, ".core")),
                                        case compile:file(Filename, [from_core, binary, return_errors]) of
                                            {ok, ModName, Binary} ->
                                                code:purge(ModName),
                                                case code:load_binary(ModName, Filename, Binary) of
                                                    {module, ModName} ->
                                                        Val = ModName:'__eval__'(),
                                                        code:purge(ModName),
                                                        code:delete(ModName),
                                                        {ok, Val};
                                                    {error, What} ->
                                                        {error, {load_failed, What}}
                                                end;
                                            {error, Errors, _Warnings} ->
                                                {error, {compile_failed, Errors}}
                                        end
                                    catch
                                        Class:Reason:Stack ->
                                            {error, {Class, Reason, Stack}}
                                    end,
                                    io:format("~s~n", [<<0, "DREAM_RESULT", 0>>]),
                                    case Result of
                                        {ok, Value} -> io:format("ok:~p~n", [Value]);
                                        {error, Err} -> io:format("err:~p~n", [Err])
                                    end,
                                    Loop()
                            end;
                        ModStr ->
                            Paths = code:get_path(),
                            BeamFile = ModStr ++ ".beam",
                            FindFile = fun FindFile([]) -> not_found;
                                           FindFile([Dir|Rest]) ->
                                               Path = filename:join(Dir, BeamFile),
                                               case filelib:is_file(Path) of
                                                   true -> {ok, Path};
                                                   false -> FindFile(Rest)
                                               end
                                       end,
                            case FindFile(Paths) of
                                not_found ->
                                    io:format("~s~nerr:module_not_found~n", [<<0, "DREAM_RESULT", 0>>]);
                                {ok, FullPath} ->
                                    case beam_lib:chunks(FullPath, [exports]) of
                                        {ok, {_, [{exports, Exports}]}} ->
                                            io:format("~s~nok:~w~n", [<<0, "DREAM_RESULT", 0>>, Exports]);
                                        {error, _, _} ->
                                            io:format("~s~nerr:beam_read_failed~n", [<<0, "DREAM_RESULT", 0>>])
                                    end
                            end,
                            Loop()
                    end
            end
    end
end,
Loop()
"#;

/// Keywords for completion
const KEYWORDS: &[&str] = &[
    "let", "if", "else", "match", "fn", "pub", "mod", "use", "struct", "enum",
    "trait", "impl", "type", "extern", "spawn", "send", "receive", "self",
    "true", "false",
];

/// REPL commands for completion
const COMMANDS: &[&str] = &[":help", ":quit", ":q", ":clear", ":bindings", ":b", ":h", ":reload"];

/// Information about a module's exports
#[derive(Clone, Debug, Default)]
struct ModuleInfo {
    /// Module-level functions: name -> arity
    functions: HashMap<String, u8>,
    /// Struct impl methods: method_name -> (struct_name, arity)
    /// e.g., "put" -> ("Map", 3) means Map_put/3
    struct_methods: HashMap<String, (String, u8)>,
}

/// Registry of all known modules and their exports
#[derive(Clone, Debug, Default)]
struct ModuleRegistry {
    /// Map from module name (e.g., "map", "string") to its info
    modules: HashMap<String, ModuleInfo>,
}

impl ModuleRegistry {
    fn new() -> Self {
        Self {
            modules: HashMap::new(),
        }
    }

    /// Parse exports and categorize into functions vs struct methods
    fn add_module(&mut self, module_name: &str, exports: &[(String, u8)]) {
        let mut info = ModuleInfo::default();

        // Extract the short module name (e.g., "dream::map" -> "map")
        let short_name = module_name
            .strip_prefix("dream::")
            .unwrap_or(module_name);

        // Expected struct name for this module (e.g., "map" -> "Map")
        let expected_struct = capitalize_first(short_name);
        let struct_prefix = format!("{}_", expected_struct);

        for (func_name, arity) in exports {
            // Skip internal functions
            if func_name == "module_info" {
                continue;
            }

            if let Some(method_name) = func_name.strip_prefix(&struct_prefix) {
                // This is a struct method: Map_put -> ("Map", "put", arity)
                info.struct_methods
                    .insert(method_name.to_string(), (expected_struct.clone(), *arity));
            } else {
                // This is a module-level function
                info.functions.insert(func_name.clone(), *arity);
            }
        }

        self.modules.insert(short_name.to_string(), info);
    }

    /// Get all function names for a module (for completion)
    fn get_completions(&self, module: &str) -> Vec<String> {
        let mut completions = Vec::new();
        if let Some(info) = self.modules.get(module) {
            completions.extend(info.functions.keys().cloned());
            completions.extend(info.struct_methods.keys().cloned());
        }
        completions.sort();
        completions
    }

    /// Get all module names (for completion)
    fn get_module_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.modules.keys().cloned().collect();
        names.sort();
        names
    }

    /// Resolve a method call to (module, compiled_function_name)
    /// For struct methods: m.put(...) -> ("map", "Map_put")
    /// For module functions: x.unwrap() -> ("option", "unwrap")
    fn resolve_method(&self, method: &str) -> Option<(String, String)> {
        // First, check for struct methods (they take priority for methods like "put", "get")
        for (module_name, info) in &self.modules {
            if let Some((struct_name, _arity)) = info.struct_methods.get(method) {
                let compiled_name = format!("{}_{}", struct_name, method);
                return Some((module_name.clone(), compiled_name));
            }
        }

        // Then check for module-level functions
        for (module_name, info) in &self.modules {
            if info.functions.contains_key(method) {
                return Some((module_name.clone(), method.to_string()));
            }
        }

        None
    }
}

/// Shared state type
type SharedBindings = Rc<RefCell<Vec<Binding>>>;
type SharedRegistry = Rc<RefCell<ModuleRegistry>>;

/// Binding stored from a let statement
#[derive(Clone, Debug)]
struct Binding {
    name: String,
    core_expr: String,
}

/// REPL helper for rustyline
struct ReplHelper {
    bindings: SharedBindings,
    registry: SharedRegistry,
}

impl ReplHelper {
    fn new(bindings: SharedBindings, registry: SharedRegistry) -> Self {
        Self { bindings, registry }
    }
}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let mut completions = Vec::new();
        let (start, word) = find_word_start(line, pos);

        // Complete REPL commands
        if word.starts_with(':') {
            for cmd in COMMANDS {
                if cmd.starts_with(word) {
                    completions.push(Pair {
                        display: cmd.to_string(),
                        replacement: cmd.to_string(),
                    });
                }
            }
            return Ok((start, completions));
        }

        // Complete module paths (e.g., "string::" -> "string::reverse")
        if let Some(colon_pos) = word.find("::") {
            let module = &word[..colon_pos];
            let func_prefix = &word[colon_pos + 2..];

            let registry = self.registry.borrow();
            for func in registry.get_completions(module) {
                if func.starts_with(func_prefix) {
                    let full = format!("{}::{}", module, func);
                    completions.push(Pair {
                        display: full.clone(),
                        replacement: full,
                    });
                }
            }
            return Ok((start, completions));
        }

        // Complete keywords
        for kw in KEYWORDS {
            if kw.starts_with(word) && !word.is_empty() {
                completions.push(Pair {
                    display: kw.to_string(),
                    replacement: kw.to_string(),
                });
            }
        }

        // Complete binding names
        let bindings = self.bindings.borrow();
        for binding in bindings.iter() {
            if binding.name.starts_with(word) && !word.is_empty() {
                completions.push(Pair {
                    display: binding.name.clone(),
                    replacement: binding.name.clone(),
                });
            }
        }

        // Complete module names (with :: suffix)
        let registry = self.registry.borrow();
        for module in registry.get_module_names() {
            if module.starts_with(word) && !word.is_empty() {
                completions.push(Pair {
                    display: format!("{}::", module),
                    replacement: format!("{}::", module),
                });
            }
        }

        // Also suggest struct names (capitalized modules) for Struct::method syntax
        for module in registry.get_module_names() {
            let struct_name = capitalize_first(&module);
            if struct_name.starts_with(word) && !word.is_empty() {
                completions.push(Pair {
                    display: format!("{}::", struct_name),
                    replacement: format!("{}::", struct_name),
                });
            }
        }

        Ok((start, completions))
    }
}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Highlighter for ReplHelper {}
impl Validator for ReplHelper {}
impl Helper for ReplHelper {}

/// Find the start of the word at position
fn find_word_start(line: &str, pos: usize) -> (usize, &str) {
    let line = &line[..pos];
    let start = line
        .rfind(|c: char| c.is_whitespace() || c == '(' || c == '[' || c == '{' || c == ',')
        .map(|i| i + 1)
        .unwrap_or(0);
    (start, &line[start..])
}

/// REPL state
struct ReplState {
    bindings: SharedBindings,
    registry: SharedRegistry,
    beam_process: Option<Child>,
    beam_stdin: Option<std::process::ChildStdin>,
    beam_stdout: Option<BufReader<std::process::ChildStdout>>,
    stdlib_path: Option<String>,
    temp_dir: std::path::PathBuf,
}

impl ReplState {
    fn new() -> Self {
        let stdlib_path = find_stdlib_path();
        let temp_dir = std::env::temp_dir();

        Self {
            bindings: Rc::new(RefCell::new(Vec::new())),
            registry: Rc::new(RefCell::new(ModuleRegistry::new())),
            beam_process: None,
            beam_stdin: None,
            beam_stdout: None,
            stdlib_path,
            temp_dir,
        }
    }

    /// Start the BEAM process if not already running
    fn ensure_beam_running(&mut self) -> Result<(), String> {
        if self.beam_process.is_some() {
            return Ok(());
        }

        // Minify the eval server code (remove newlines)
        let eval_code = EVAL_SERVER
            .lines()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let eval_code = format!("{}.", eval_code);

        let mut cmd = Command::new("erl");
        cmd.arg("-noshell");
        cmd.arg("-pa").arg(&self.temp_dir);

        if let Some(ref stdlib) = self.stdlib_path {
            cmd.arg("-pa").arg(stdlib);
        }

        cmd.arg("-eval").arg(&eval_code);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to start BEAM: {}", e))?;

        let stdin = child.stdin.take().ok_or("Failed to get stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to get stdout")?;

        self.beam_process = Some(child);
        self.beam_stdin = Some(stdin);
        self.beam_stdout = Some(BufReader::new(stdout));

        Ok(())
    }

    /// Send a command to the BEAM server and get the response
    fn send_command(&mut self, cmd: &str) -> Result<String, String> {
        self.ensure_beam_running()?;

        let stdin = self.beam_stdin.as_mut().ok_or("BEAM stdin not available")?;
        writeln!(stdin, "{}", cmd).map_err(|e| format!("Failed to send: {}", e))?;
        stdin.flush().map_err(|e| format!("Failed to flush: {}", e))?;

        let stdout = self.beam_stdout.as_mut().ok_or("BEAM stdout not available")?;

        // Read until we see the result marker
        loop {
            let mut line = String::new();
            stdout
                .read_line(&mut line)
                .map_err(|e| format!("Failed to read: {}", e))?;

            if line.is_empty() {
                return Err("No response from BEAM".to_string());
            }

            let trimmed = line.trim();
            if trimmed == RESULT_MARKER {
                break;
            }

            // Print any output (from io::println etc)
            print!("{}", line);
        }

        // Read the result line
        let mut result_line = String::new();
        stdout
            .read_line(&mut result_line)
            .map_err(|e| format!("Failed to read result: {}", e))?;

        let result_line = result_line.trim();
        if let Some(value) = result_line.strip_prefix("ok:") {
            Ok(value.to_string())
        } else if let Some(err) = result_line.strip_prefix("err:") {
            Err(err.to_string())
        } else {
            Err(format!("Invalid result: {}", result_line))
        }
    }

    /// Load module registry by introspecting the BEAM
    fn load_registry(&mut self) -> Result<(), String> {
        // Get list of dream:: modules
        let modules_str = self.send_command("introspect:list_modules")?;

        // Parse the module list: ['dream::map', 'dream::string', ...]
        let modules = parse_atom_list(&modules_str);

        for module in modules {
            // Get exports for each module
            let cmd = format!("introspect:exports:{}", module);
            if let Ok(exports_str) = self.send_command(&cmd) {
                let exports = parse_exports(&exports_str);
                self.registry.borrow_mut().add_module(&module, &exports);
            }
        }

        Ok(())
    }

    /// Evaluate an expression
    fn eval_expr(&mut self, expr: &Expr) -> Result<String, String> {
        let counter = EVAL_COUNTER.fetch_add(1, Ordering::SeqCst);
        let module_name = format!("dream_repl_{}", counter);
        let core_erlang = self.generate_core_erlang(&module_name, expr)?;

        let core_file = self.temp_dir.join(format!("{}.core", module_name));
        std::fs::write(&core_file, &core_erlang)
            .map_err(|e| format!("Failed to write Core Erlang: {}", e))?;

        let cmd = format!("eval:{}", core_file.display());
        let result = self.send_command(&cmd);

        let _ = std::fs::remove_file(&core_file);

        result.map(|v| format_dream_value(&v))
    }

    /// Generate Core Erlang for an expression
    fn generate_core_erlang(&self, module_name: &str, expr: &Expr) -> Result<String, String> {
        let mut output = String::new();
        output.push_str(&format!(
            "module '{}' ['__eval__'/0]\n    attributes []\n\n'__eval__'/0 =\nfun () ->\n",
            module_name
        ));

        let bindings = self.bindings.borrow();
        for binding in bindings.iter() {
            output.push_str(&format!(
                "    let <{}> =\n    {}\n    in ",
                capitalize_first(&binding.name),
                binding.core_expr
            ));
        }

        let registry = self.registry.borrow();
        let expr_core = expr_to_core(expr, &bindings, &registry)?;
        output.push_str(&expr_core);
        output.push_str("\nend\n");

        Ok(output)
    }

    fn add_binding(&mut self, name: String, expr: &Expr) -> Result<(), String> {
        let bindings = self.bindings.borrow();
        let registry = self.registry.borrow();
        let core_expr = expr_to_core(expr, &bindings, &registry)?;
        drop(bindings);
        drop(registry);

        let mut bindings = self.bindings.borrow_mut();
        bindings.retain(|b| b.name != name);
        bindings.push(Binding { name, core_expr });
        Ok(())
    }

    fn clear_bindings(&mut self) {
        self.bindings.borrow_mut().clear();
    }
}

impl Drop for ReplState {
    fn drop(&mut self) {
        self.beam_stdin.take();
        if let Some(ref mut child) = self.beam_process {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Convert an expression to Core Erlang
fn expr_to_core(expr: &Expr, bindings: &[Binding], registry: &ModuleRegistry) -> Result<String, String> {
    match expr {
        Expr::Int(n) => Ok(n.to_string()),
        Expr::Bool(b) => Ok(if *b { "'true'" } else { "'false'" }.to_string()),
        Expr::String(s) => {
            let chars: Vec<String> = s.bytes().map(|b| b.to_string()).collect();
            Ok(format!("[{}]", chars.join(", ")))
        }
        Expr::Atom(a) => Ok(format!("'{}'", a)),
        Expr::Ident(name) => {
            if bindings.iter().any(|b| &b.name == name) {
                Ok(capitalize_first(name))
            } else {
                Err(format!("Undefined variable: {}", name))
            }
        }
        Expr::Binary { op, left, right } => {
            let left_core = expr_to_core(left, bindings, registry)?;
            let right_core = expr_to_core(right, bindings, registry)?;
            let op_str = match op {
                BinOp::Add => "call 'erlang':'+'",
                BinOp::Sub => "call 'erlang':'-'",
                BinOp::Mul => "call 'erlang':'*'",
                BinOp::Div => "call 'erlang':'div'",
                BinOp::Mod => "call 'erlang':'rem'",
                BinOp::Eq => "call 'erlang':'=:='",
                BinOp::Ne => "call 'erlang':'=/='",
                BinOp::Lt => "call 'erlang':'<'",
                BinOp::Le => "call 'erlang':'=<'",
                BinOp::Gt => "call 'erlang':'>'",
                BinOp::Ge => "call 'erlang':'>='",
                BinOp::And => "call 'erlang':'and'",
                BinOp::Or => "call 'erlang':'or'",
            };
            Ok(format!("{}({}, {})", op_str, left_core, right_core))
        }
        Expr::Tuple(elems) => {
            let elem_strs: Result<Vec<_>, _> = elems
                .iter()
                .map(|e| expr_to_core(e, bindings, registry))
                .collect();
            Ok(format!("{{{}}}", elem_strs?.join(", ")))
        }
        Expr::List(elems) => {
            let elem_strs: Result<Vec<_>, _> = elems
                .iter()
                .map(|e| expr_to_core(e, bindings, registry))
                .collect();
            Ok(format!("[{}]", elem_strs?.join(", ")))
        }
        Expr::Call { func, args, .. } => {
            if let Expr::Path { segments } = func.as_ref() {
                if segments.len() == 2 {
                    let first = &segments[0];
                    let func_name = &segments[1];
                    let arg_strs: Result<Vec<_>, _> = args
                        .iter()
                        .map(|a| expr_to_core(a, bindings, registry))
                        .collect();

                    // Check if first segment is PascalCase (struct) or snake_case (module)
                    let is_struct = first.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);

                    if is_struct {
                        let module = first.to_lowercase();
                        let compiled_func = format!("{}_{}", first, func_name);
                        return Ok(format!(
                            "call 'dream::{}'  :'{}'({})",
                            module, compiled_func, arg_strs?.join(", ")
                        ));
                    } else {
                        return Ok(format!(
                            "call 'dream::{}'  :'{}'({})",
                            first, func_name, arg_strs?.join(", ")
                        ));
                    }
                }
            }

            if let Expr::Ident(name) = func.as_ref() {
                let arg_strs: Result<Vec<_>, _> = args
                    .iter()
                    .map(|a| expr_to_core(a, bindings, registry))
                    .collect();
                return Ok(format!("apply '{}'({})", name, arg_strs?.join(", ")));
            }

            Err(format!("Unsupported call: {:?}", func))
        }
        Expr::MethodCall { receiver, method, args, .. } => {
            let receiver_core = expr_to_core(receiver, bindings, registry)?;
            let mut all_args = vec![receiver_core];
            for arg in args {
                all_args.push(expr_to_core(arg, bindings, registry)?);
            }

            // Use the registry to resolve the method
            if let Some((module, compiled_func)) = registry.resolve_method(method) {
                return Ok(format!(
                    "call 'dream::{}'  :'{}'({})",
                    module, compiled_func, all_args.join(", ")
                ));
            }

            Err(format!(
                "Unknown method '{}'. Use Module::method() syntax.",
                method
            ))
        }
        Expr::StringInterpolation(parts) => {
            let mut part_strs = Vec::new();
            for part in parts {
                match part {
                    StringPart::Literal(s) => {
                        let chars: Vec<String> = s.chars().map(|c| (c as u32).to_string()).collect();
                        part_strs.push(format!("[{}]", chars.join(", ")));
                    }
                    StringPart::Expr(e) => {
                        let expr_core = expr_to_core(e, bindings, registry)?;
                        part_strs.push(format!("call 'dream::display':'to_string'({})", expr_core));
                    }
                }
            }
            Ok(format!(
                "call 'erlang':'iolist_to_binary'([{}])",
                part_strs.join(", ")
            ))
        }
        Expr::Pipe { left, right } => {
            let left_core = expr_to_core(left, bindings, registry)?;

            match right.as_ref() {
                Expr::Call { func, args, .. } => {
                    let mut all_args = vec![left_core];
                    for arg in args {
                        all_args.push(expr_to_core(arg, bindings, registry)?);
                    }

                    if let Expr::Path { segments } = func.as_ref() {
                        if segments.len() == 2 {
                            let first = &segments[0];
                            let func_name = &segments[1];
                            let is_struct = first.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);

                            if is_struct {
                                let module = first.to_lowercase();
                                let compiled_func = format!("{}_{}", first, func_name);
                                return Ok(format!(
                                    "call 'dream::{}'  :'{}'({})",
                                    module, compiled_func, all_args.join(", ")
                                ));
                            } else {
                                return Ok(format!(
                                    "call 'dream::{}'  :'{}'({})",
                                    first, func_name, all_args.join(", ")
                                ));
                            }
                        }
                    }

                    if let Expr::Ident(name) = func.as_ref() {
                        return Ok(format!("apply '{}'({})", name, all_args.join(", ")));
                    }

                    Err(format!("Unsupported pipe target: {:?}", func))
                }
                Expr::MethodCall { method, args, .. } => {
                    let mut all_args = vec![left_core];
                    for arg in args {
                        all_args.push(expr_to_core(arg, bindings, registry)?);
                    }

                    if let Some((module, compiled_func)) = registry.resolve_method(method) {
                        return Ok(format!(
                            "call 'dream::{}'  :'{}'({})",
                            module, compiled_func, all_args.join(", ")
                        ));
                    }

                    Err(format!("Unknown method '{}'", method))
                }
                _ => Err(format!("Pipe right side must be a call: {:?}", right)),
            }
        }
        _ => Err(format!("Unsupported expression: {:?}", expr)),
    }
}

/// Parse a list of atoms from Erlang format: ['atom1', 'atom2', ...]
fn parse_atom_list(s: &str) -> Vec<String> {
    let s = s.trim();
    if s == "[]" {
        return Vec::new();
    }

    let inner = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(s);

    inner
        .split(',')
        .filter_map(|atom| {
            let atom = atom.trim();
            atom.strip_prefix('\'')
                .and_then(|a| a.strip_suffix('\''))
                .map(|a| a.to_string())
        })
        .collect()
}

/// Parse exports from Erlang format: [{func, arity}, ...]
fn parse_exports(s: &str) -> Vec<(String, u8)> {
    let s = s.trim();
    if s == "[]" {
        return Vec::new();
    }

    let mut exports = Vec::new();
    let inner = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(s);

    // Simple parser for {name, arity} tuples
    let mut i = 0;
    let chars: Vec<char> = inner.chars().collect();

    while i < chars.len() {
        if chars[i] == '{' {
            let start = i + 1;
            while i < chars.len() && chars[i] != '}' {
                i += 1;
            }
            let tuple_content = &inner[start..i];
            if let Some((name, arity)) = parse_export_tuple(tuple_content) {
                exports.push((name, arity));
            }
        }
        i += 1;
    }

    exports
}

fn parse_export_tuple(s: &str) -> Option<(String, u8)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return None;
    }

    let name = parts[0].trim();
    let name = name.strip_prefix('\'').and_then(|n| n.strip_suffix('\''))
        .unwrap_or(name);

    let arity: u8 = parts[1].trim().parse().ok()?;

    Some((name.to_string(), arity))
}

/// Find stdlib path
fn find_stdlib_path() -> Option<String> {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let stdlib = exe_dir.join("../stdlib");
            if stdlib.exists() {
                return stdlib.canonicalize().ok().map(|p| p.to_string_lossy().into_owned());
            }
        }
    }

    let target_stdlib = std::path::Path::new("target/stdlib");
    if target_stdlib.exists() {
        return target_stdlib.canonicalize().ok().map(|p| p.to_string_lossy().into_owned());
    }

    None
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Format Erlang value as Dream syntax
fn format_dream_value(value: &str) -> String {
    let value = value.trim();

    if value == "none" {
        return "None".to_string();
    }
    if let Some(inner) = value.strip_prefix("{some,").and_then(|s| s.strip_suffix('}')) {
        return format!("Some({})", format_dream_value(inner.trim()));
    }
    if let Some(inner) = value.strip_prefix("{ok,").and_then(|s| s.strip_suffix('}')) {
        return format!("Ok({})", format_dream_value(inner.trim()));
    }
    if let Some(inner) = value.strip_prefix("{error,").and_then(|s| s.strip_suffix('}')) {
        return format!("Err({})", format_dream_value(inner.trim()));
    }

    if let Some(inner) = value.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        if inner == "true" || inner == "false" || inner == "ok" || inner == "error" {
            return inner.to_string();
        }
        return format!(":{}", inner);
    }

    if let Some(inner) = value.strip_prefix("<<\"").and_then(|s| s.strip_suffix("\">>")) {
        return format!("\"{}\"", inner);
    }

    if value.starts_with("#{") && value.contains("'__struct__'") {
        if let Some(struct_start) = value.find("'__struct__' => '") {
            let after_struct = &value[struct_start + 17..];
            if let Some(struct_end) = after_struct.find('\'') {
                let struct_path = &after_struct[..struct_end];
                let struct_name = struct_path.split("::").last().unwrap_or(struct_path);

                if let Some(data_start) = value.find("data => ") {
                    let after_data = &value[data_start + 8..];
                    if let Some(data_content) = extract_map_content(after_data) {
                        if struct_name == "Map" {
                            if data_content == "#{}" {
                                return "{}".to_string();
                            }
                            return format!("{{ {} }}", format_map_fields(&data_content));
                        }
                        if data_content == "#{}" {
                            return format!("{}()", struct_name);
                        }
                        return format!("{}({})", struct_name, format_map_fields(&data_content));
                    }
                }
            }
        }
    }

    value.to_string()
}

fn extract_map_content(s: &str) -> Option<String> {
    if !s.starts_with("#{") {
        return None;
    }
    let mut depth = 0;
    let mut end_idx = 0;
    for (i, c) in s.chars().enumerate() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end_idx = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if end_idx > 0 { Some(s[..end_idx].to_string()) } else { None }
}

fn format_map_fields(map_str: &str) -> String {
    let inner = map_str.strip_prefix("#{").and_then(|s| s.strip_suffix('}'));
    if let Some(inner) = inner {
        if inner.is_empty() { return String::new(); }
        inner.replace(" => ", ": ")
    } else {
        map_str.to_string()
    }
}

fn print_banner() {
    println!("Dream {} (BEAM backend)", env!("CARGO_PKG_VERSION"));
    println!("Type :help for commands, :quit to exit");
    println!();
}

fn print_help() {
    println!("Commands:");
    println!("  :help          Show this help message");
    println!("  :quit, :q      Exit the shell");
    println!("  :clear         Clear all bindings");
    println!("  :bindings      Show current bindings");
    println!("  :reload        Reload module registry");
    println!();
    println!("Enter Dream expressions to evaluate them.");
    println!("Use 'let x = expr' to create bindings.");
    println!("Press TAB for completion.");
}

/// Run the interactive shell
pub fn run_shell() -> ExitCode {
    print_banner();

    let mut state = ReplState::new();

    // Initialize BEAM and load module registry
    print!("Loading modules...");
    std::io::stdout().flush().ok();

    if let Err(e) = state.load_registry() {
        println!(" failed: {}", e);
    } else {
        let count = state.registry.borrow().modules.len();
        println!(" {} modules loaded.", count);
    }
    println!();

    let helper = ReplHelper::new(Rc::clone(&state.bindings), Rc::clone(&state.registry));
    let mut rl = match Editor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("Failed to initialize readline: {}", e);
            return ExitCode::from(1);
        }
    };
    rl.set_helper(Some(helper));

    loop {
        let readline = rl.readline("dream> ");
        match readline {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(line);

                if line.starts_with(':') {
                    match line {
                        ":quit" | ":q" => {
                            println!("Goodbye!");
                            break;
                        }
                        ":help" | ":h" => {
                            print_help();
                            continue;
                        }
                        ":clear" => {
                            state.clear_bindings();
                            println!("Bindings cleared.");
                            continue;
                        }
                        ":bindings" | ":b" => {
                            let bindings = state.bindings.borrow();
                            if bindings.is_empty() {
                                println!("No bindings.");
                            } else {
                                for binding in bindings.iter() {
                                    println!("  {}", binding.name);
                                }
                            }
                            continue;
                        }
                        ":reload" => {
                            print!("Reloading modules...");
                            std::io::stdout().flush().ok();
                            state.registry.borrow_mut().modules.clear();
                            if let Err(e) = state.load_registry() {
                                println!(" failed: {}", e);
                            } else {
                                let count = state.registry.borrow().modules.len();
                                println!(" {} modules loaded.", count);
                            }
                            continue;
                        }
                        _ => {}
                    }
                }

                match parse_and_eval(&mut state, line) {
                    Ok(result) => println!("{}", result),
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
    }

    ExitCode::SUCCESS
}

fn parse_and_eval(state: &mut ReplState, input: &str) -> Result<String, String> {
    if input.trim_start().starts_with("let ") {
        return parse_and_eval_let(state, input);
    }

    let mut parser = Parser::new(input);
    let expr = parser.parse_expr().map_err(|e| format!("Parse error: {:?}", e))?;
    state.eval_expr(&expr)
}

fn parse_and_eval_let(state: &mut ReplState, input: &str) -> Result<String, String> {
    let input = input.trim_start().strip_prefix("let ").unwrap();
    let eq_pos = input.find('=').ok_or("Expected '=' in let statement")?;

    let name = input[..eq_pos].trim().to_string();
    let expr_str = input[eq_pos + 1..].trim();

    if name.is_empty() || !name.chars().next().unwrap().is_alphabetic() {
        return Err("Invalid variable name".to_string());
    }

    let mut parser = Parser::new(expr_str);
    let expr = parser.parse_expr().map_err(|e| format!("Parse error: {:?}", e))?;

    state.add_binding(name, &expr)?;
    Ok(":ok".to_string())
}
