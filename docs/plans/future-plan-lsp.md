# Plan: Language Server (LSP) for VS Code

Status: **Proposed** — not yet implemented.

## Goal

Provide a modern editor experience for the Red clone in Visual Studio Code via a Language Server Protocol implementation. Ship as a new Rust binary crate (`red-lsp`) plus a minimal VS Code extension. MVP scope first; navigation and refactoring features phased in later.

## Decisions (confirmed)

| Decision | Choice |
|---|---|
| LSP framework | `tower-lsp` + `tokio` |
| Feature scope (v1) | MVP: diagnostics, completion, hover, signatureHelp, documentSymbol |
| Binary layout | Separate `red-lsp` binary crate (keeps `red-cli` lean) |
| Parser recovery | Add a recovery heuristic in `red-lsp`; core parser stays strict |

## Existing foundations (no work needed)

The codebase already has everything an LSP needs:

- **Span tracking is pervasive.** Every source-origin `Value` carries `Span{start,end}` byte offsets (lexer emits them; parser threads them through; eval errors propagate them). `Error::span() -> Option<Span>` gives a uniform diagnostic-source accessor.
- **`LineMap`** in `crates/red-core/src/source.rs` converts byte offsets → `(line, col)` in `O(log lines)` — exactly what LSP `Position` needs.
- **Pure, separable lex/parse** — `lex(&str) -> Result<Vec<Token>, LexError>` and `load(&[Token]) -> Result<Series, ParseError>` have no I/O or global state, so they are cheap to run per-keystroke.
- **Binding resolution** — `Binding::{Local, Func, Lexical, Closure}` on every word enables goto-definition/references/hover by walking the bound tree.
- **Enumerable native registry** — `register_natives(&mut Env)` inserts ~140 natives; `install_constants` seeds `none`/`true`/`false`/`newline`/`system`; `FuncDef.params`/`refinements` give arity + refinement signatures — a ready source for completion items and signature help.
- **Stdlib** (`crates/red-eval/stdlib/stdlib.red`) adds ~25 more words.
- **Printer round-trips** — `mold(parse(s)) == normalize(s)`, useful for hover/formatting.

### Constraint to design around

`Env` is `!Send` (`Rc<RefCell<…>>` everywhere). The language server cannot share an `Env` across threads. Solution: run the engine on a **single dedicated worker thread** and serialize all document work through it via a channel of `Box<dyn FnOnce + Send>` closures. The async LSP runtime (`tokio`) only handles JSON-RPC I/O.

## Architecture

### Workspace changes

Add `crates/red-lsp/` to the workspace (`Cargo.toml` `members`).

- Binary crate (`[[bin]] name = "red-lsp"`).
- Dependencies: `red-core` (path), `red-eval` (path), `tower-lsp = "0.20"`, `tokio = { version = "1", features = ["full"] }`, `serde_json`, `anyhow`, `log` + `env_logger`.
- Dev-dependencies: `tower-lsp` test helpers, `tempfile`.

### Source layout

```
crates/red-lsp/src/
├── main.rs              # entry, tokio::main, env_logger, Backend::run()
├── server.rs            # #[tower_lsp::async_trait] impl LanguageServer for Backend
├── state.rs             # WorldState { documents, worker handle, ... }
├── document.rs          # Document { text, linemap, tokens, tree, version }
├── worker.rs            # single dedicated thread + mpsc channel of Box<dyn FnOnce + Send>
├── capabilities.rs      # declares server capabilities
├── to_span.rs           # Span <-> LSP Range conversions using LineMap
├── diagnostics.rs       # Error -> Diagnostic (uses Error::span())
├── recovery.rs          # parse-recovery heuristic
├── features/
│   ├── mod.rs
│   ├── completion.rs    # natives + constants + stdlib + locals -> CompletionItem[]
│   ├── hover.rs         # resolve Binding, mold value / FuncDef spec -> Hover
│   ├── signature.rs     # FuncDef.params + refinements -> SignatureHelp
│   └── symbols.rs       # top-level SetWord/SetPath/func/object/module -> DocumentSymbol[]
└── stdlib_index.rs      # parse stdlib.red once at startup to extract word list + docstrings
```

### Threading model

- `main.rs` spawns the LSP on a multi-threaded tokio runtime for I/O.
- `worker.rs` owns **one** std thread running `std::sync::mpsc::Receiver<Box<dyn FnOnce(&WorkerCtx) + Send>>`. All engine interaction happens inside closures run on that thread — each closure receives a per-server `WorkerCtx` holding a freshly-built `Env` (via `register_natives` + `install_constants` + stdlib load at startup) plus the `HashMap<Uri, Document>`.
- LSP handlers are async; they `.blocking_send(msg).await` to the worker and await the reply. Single point of serialization; no `Send` bounds on `Env` needed.

### Document sync

- `textDocument/didOpen` + `didChange` (`full` sync for MVP — small docs; `Incremental` later): store text, rebuild `LineMap`, run `lex` + recovery-augmented `load`, store `Result<Series, (Series, Vec<Error>)>`.
- On change: debounce via `tokio::time::sleep` (e.g. 150ms), then re-analyze and `publishDiagnostics`.
- `didClose`/`didSave` minimal for MVP.

### Parser recovery (`recovery.rs`)

- Wrap `red_core::parser::load` in a recovery loop: on `ParseError`, record it, skip tokens until re-sync (balanced `[](){}`, or a top-level `SetWord`), then resume. Returns `(partial_series: Series, errors: Vec<Error>)`. Keeps symbols/completion working mid-edit.
- Stays in `red-lsp`; `red-core` parser remains strict.

## MVP feature set (Phase 1)

| Capability | Source |
|---|---|
| `publishDiagnostics` | `lex` + recovery `load` → `Error::span()` + `LineMap::line_col` → `Diagnostic { range, severity, message, source: "red" }` |
| `documentSymbol` | walk partial `Series`; collect top-level `SetWord`/`SetPath`, `func`/`does`/`closure`/`function` calls, `object`/`module` blocks; each carries a `Span` |
| `hover` | word/path under cursor → resolve `Binding` → `mold` the bound value or `FuncDef` spec (params/refinements) → markdown |
| `completion` | trigger on word char; merge: `register_natives` keys (~140), `install_constants` (none/true/false/newline/system), stdlib words (~25), document `SetWord`s in scope. `FuncDef` params → `CompletionItemKind.Function` w/ detail |
| `signatureHelp` | when cursor inside a call's arg list; match callee name → `FuncDef.params`/`refinements` → `SignatureInformation { label, parameters, activeParameter }` |

## VS Code extension

```
editors/vscode/
├── package.json              # engines.vscode ^1.75, activationOnLanguage:red, contributes.{languages,grammars,configuration}, main: ./out/extension.js
├── language-configuration.json  # brackets [](){}, autoClosing pairs incl "" {}, comments ";", surroundingPairs
├── syntaxes/red.tmLanguage.json # TextMate grammar (basic; independent of LSP)
├── src/extension.ts          # vscode-languageclient: spawn `red-lsp` binary from bin/ via config path
├── tsconfig.json
├── .vscodeignore
└── README.md
```

- `vscode-languageclient` spawns the bundled `red-lsp` binary (path configurable, defaults to extension's `bin/`).
- Extension packaging via `npm` + `vsce`; `Justfile` gains `lsp` (build `red-lsp` + copy to `editors/vscode/bin/`) and `lsp-vscode-package` recipes.

## Future phases (out of MVP scope)

### Phase 2 — Navigation

- `textDocument/definition` + `references` — walk bound tree matching `Symbol` + `Context` slot (for `Local`/`Func`).
- `textDocument/documentLink` — file/url literals.
- `textDocument/selectionRange` — block/paren nesting from spans.
- `textDocument/semanticTokens` — token-kind → LSP token type; lexer already classifies.

### Phase 3 — Refactoring & formatting

- `textDocument/formatting` — leverage `mold` round-trip property.
- `textDocument/rename` — binding-aware.
- `textDocument/codeAction` — e.g. wrap/unwrap blocks.
- `textDocument/foldingRange` — block/paren/string spans.

## Build / dev loop

- `just lsp` → `cargo build -p red-lsp --release` + copy binary to `editors/vscode/bin/red-lsp`.
- `just check` extended to typecheck/build the new crate (no new lint config).
- `cargo test -p red-lsp` for recovery + span-conversion + completion-merge unit tests.
- F5 in VS Code (`.vscode/launch.json` optional) launches an Extension Development Host pointed at the local `bin/`.

## API surface the LSP will consume

All public re-exports are visible in `crates/red-core/src/lib.rs` and `crates/red-eval/src/lib.rs`:

```
red_core::lexer::lex(&str) -> Result<Vec<Token>, LexError>
red_core::parser::{load, parse_program, load_source}
red_core::value::{Value, Span, Symbol, Series, Binding, FuncDef}
red_core::source::LineMap::new(&str) + line_col(usize) -> (usize, usize)
red_core::error::{Error, render_error}  + Error::span()
red_core::printer::{mold, mold_to_string, form, form_to_string}
red_core::env::{Env, EvalError, EvalMode, NativeFn, RefineArgs}
red_core::context::Context
red_eval::binding::bind_pass(&Series, Context) -> Rc<Context>
red_eval::natives::{register_natives(&mut Env), install_constants(&Context)}
red_eval::interp::eval(&Value, &mut Env) -> Result<Value, EvalError>
red_eval::{run_source_with_exit_opts, RunOptions}   # full pipeline
```

## Open follow-ups (to confirm during build)

1. **Stdlib docstrings**: does `crates/red-eval/stdlib/stdlib.red` carry `; doc ...` comments or `{...}` doc strings to mine for hover/completion `documentation`? Check while implementing.
2. **Native help text**: `FuncDef` has no `help:` field today — hover for natives will show the spec block (`mold` of params/refinements). If richer docs are wanted, add a small static `docs: HashMap<Symbol, &str>` table in `red-lsp` (optional).
3. **Cross-document goto/references**: deferred to Phase 2 per the MVP scope choice.
