# Dream

A programming language with Rust-like syntax and Erlang-style concurrency.

## Goal

Combine Rust's familiar syntax with Erlang's battle-tested concurrency model (processes, message passing, pattern matching).

## Compilation Targets

### BEAM (via Core Erlang)
```
Source → Lexer → Parser → AST → Core Erlang → BEAM
```

### WebAssembly (via Dream VM)
```
Source → Lexer → Parser → AST → Dream Bytecode → Dream VM (Rust/WASM)
```

## Language Features

- **Processes**: spawn, spawn_link, self()
- **Message Passing**: send (`!`), receive, receive with timeout
- **Links & Monitors**: bidirectional crash notification, one-way DOWN messages
- **Process Registry**: register/unregister/whereis named processes
- **Data Types**: integers, atoms, tuples, lists, strings, PIDs, binaries
- **Pattern Matching**: wildcards, variables, literals, tuple/list/struct destructuring, guards
- **Structs & Enums**: Rust-style type definitions
- **Traits**: trait definitions and implementations
- **Pipe Operator**: `|>` for function chaining

## Building

```bash
# Run tests
cargo test

# Build the compiler
cargo build --release

# Build WASM target
wasm-pack build --target web
```

## Commands

- `cargo test` - run all tests
- `cargo check` - type check
- `cargo build --release` - build compiler
- `wasm-pack build --target web` - build WASM package

---

# Agent Instructions

This project uses **bd** (beads) for issue tracking. Run `bd onboard` to get started.

## Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --status in_progress  # Claim work
bd close <id>         # Complete work
bd sync               # Sync with git
```

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
