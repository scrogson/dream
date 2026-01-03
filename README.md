# Dream

A programming language with Rust-like syntax that compiles to Core Erlang and runs on the BEAM.

Dream combines Rust's familiar syntax with Erlang's battle-tested concurrency model - processes, message passing, and pattern matching - all running on the BEAM virtual machine.

## Architecture

```
Source Code → Lexer → Parser → AST → Codegen → Core Erlang → BEAM
```

## Features

### Concurrency
- **Processes**: lightweight, isolated units of execution with parent tracking
- **Message Passing**: async send, receive with timeout, selective receive
- **Links**: bidirectional crash notification between processes
- **Monitors**: one-way crash notification (DOWN messages)
- **Process Registry**: register/unregister/whereis for named processes

### Data Types
- Integers (arbitrary precision)
- Atoms (`:ok`, `:error`)
- Tuples and Lists
- Strings
- PIDs (process identifiers)
- Binaries/Bitstrings

### Pattern Matching
- Wildcards (`_`)
- Variable binding
- Literal matching
- Tuple/list destructuring
- Struct patterns
- Guard clauses

## Example

```rust
mod ping_pong {
    pub fn start() {
        let pong = spawn || { pong_loop() };
        let ping = spawn || { ping_loop(pong, 3) };
    }

    fn ping_loop(pong: Pid, count: int) {
        if count > 0 {
            pong ! (:ping, self());
            receive {
                :pong => ping_loop(pong, count - 1)
            }
        }
    }

    fn pong_loop() {
        receive {
            (:ping, sender) => {
                sender ! :pong;
                pong_loop()
            }
        }
    }
}
```

## Building

```bash
# Run tests
cargo test

# Build the compiler
cargo build --release
```

## Usage

```bash
# Compile Dream source to Core Erlang
dream compile source.dream -o source.core

# Compile Core Erlang to BEAM bytecode
erlc +from_core source.core
```

## Project Structure

```
src/
├── lib.rs              # Public API
├── main.rs             # CLI entry point
└── compiler/
    ├── lexer.rs        # Tokenizer
    ├── parser.rs       # Parser
    ├── ast.rs          # Abstract syntax tree
    └── core_erlang.rs  # Core Erlang code generator
```

## Related Projects

- [tree-sitter-dream](https://github.com/scrogson/tree-sitter-dream) - Tree-sitter grammar for editor support

## License

MIT
