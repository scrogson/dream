# tree-sitter-toybeam

Tree-sitter grammar for the ToyBEAM language.

## Building

```bash
cd tree-sitter-toybeam
npm install
npx tree-sitter generate
```

## Testing the Parser

```bash
npx tree-sitter parse ../examples/math.tb
```

## Neovim Setup

### 1. Install nvim-treesitter

Make sure you have [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) installed.

### 2. Register the parser

Add this to your Neovim config (e.g., `~/.config/nvim/lua/plugins/treesitter.lua` or init.lua):

```lua
local parser_config = require("nvim-treesitter.parsers").get_parser_configs()

parser_config.toybeam = {
  install_info = {
    url = "~/github/scrogson/toybeam/tree-sitter-toybeam", -- adjust to your path
    files = { "src/parser.c" },
    generate_requires_npm = true,
  },
  filetype = "toybeam",
}
```

### 3. Set up filetype detection

Create `~/.config/nvim/ftdetect/toybeam.vim`:

```vim
au BufRead,BufNewFile *.tb set filetype=toybeam
```

Or in Lua (`~/.config/nvim/lua/config/filetype.lua`):

```lua
vim.filetype.add({
  extension = {
    tb = "toybeam",
  },
})
```

### 4. Copy query files

Copy the query files to nvim-treesitter's queries directory:

```bash
mkdir -p ~/.config/nvim/queries/toybeam
cp queries/*.scm ~/.config/nvim/queries/toybeam/
```

### 5. Install the parser

In Neovim, run:

```vim
:TSInstall toybeam
```

Or install manually:

```bash
cd tree-sitter-toybeam
npm run generate
```

Then in Neovim:

```vim
:TSInstallFromGrammar toybeam
```

### 6. Verify

Open a `.tb` file and run:

```vim
:InspectTree
```

You should see the parsed syntax tree.

## Testing

Create test files in `test/corpus/`:

```bash
mkdir -p test/corpus
```

Example test file (`test/corpus/functions.txt`):

```
================================================================================
Simple function
================================================================================

mod test {
    pub fn add(x: int, y: int) -> int {
        x + y
    }
}

--------------------------------------------------------------------------------

(source_file
  (module
    name: (identifier)
    (function_definition
      (visibility)
      name: (identifier)
      (parameters
        (parameter
          pattern: (identifier)
          type: (primitive_type))
        (parameter
          pattern: (identifier)
          type: (primitive_type)))
      return_type: (primitive_type)
      body: (block
        (binary_expression
          left: (identifier)
          right: (identifier))))))
```

Run tests:

```bash
npm test
```

## Syntax Highlighting

The grammar provides highlighting for:

- Keywords (`fn`, `let`, `if`, `match`, `struct`, `enum`, etc.)
- Operators (`+`, `-`, `*`, `/`, `==`, `!=`, `&&`, `||`, etc.)
- Binary syntax (`<<`, `>>`, `big`, `little`, `signed`, etc.)
- Literals (integers, strings, atoms, booleans)
- Types (type identifiers, primitives)
- Functions (definitions and calls)
- Comments (line `//` and block `/* */`)
- And more!
