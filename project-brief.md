# Plan: Proof-of-Concept Red Clone in Rust

> **Status (v0.7):** The v0.2 language surface (lexer/parser/evaluator/
> series/binding/functions/`parse`/refinements/paths/objects/I/O) shipped as
> `v0.2.0-poc`. v0.3 added a **bytecode compiler + stack VM** (the default
> evaluator), lexical addressing, tail-call optimization, a disassembler
> (`--disasm`), per-instr tracing (`--trace`), and property tests + fuzzing.
> v0.4 re-opened the language surface (`char!`/`binary!`/`map!`/`pair!`/
> `tuple!`/`date!`/`bitset!`, `compose`, trig, the full `error!` model, the
> completed `parse` dialect). v0.5 adds **first-class closures** (`closure!`
> with snapshot freevar capture) and **modules** (`module`/`export`/`import`,
> with named-module caching, file-based import, and a small auto-imported
> stdlib). v0.6 closed **core functional gaps** (`parse` named-rule recursion,
> `mold` native, series `sort` + set ops, `port!` + minimal synchronous HTTP
> networking). v0.7 is a **type-completeness release**: nine new `Value`
> variants (`percent!`/`money!`/`issue!`/`email!`/`tag!`/`unset!`/`hash!`/
> `vector!`/`image!`/`typeset!`), the `native!`/`op!` type split, a gated
> `--unset-on-unbound` fallback, and `typeset!`-backed typed-func arg
> type-checking. `regex!`/`struct!`/`handle!` deferred to v0.8. The
> tree-walker (`interp_walker.rs`) is retained as the `--walk` fallback for
> `needs_rebind`-flagged blocks (`use`/`make object!`/foreign-bound) and for
> parity comparison (`--features force-walk`). v0.11 adds the `duration!`
> value type — a signed span-of-time scalar backed by `chrono::Duration`
> (i64 nanoseconds, ~292-year range), with both single-unit (`30s`/`1.5h`/
> `250ms`) and compound (`1d1h`/`1h30m45s`) literal forms, `date - date →
> duration!` (replacing the stopgap `integer!` day count), and strict-typed
> arithmetic (not a `number!` — mirrors `money!`'s precedent). v0.11 also
> adds `decimal!` — a true fixed-decimal numeric backed by `rust_decimal`
> (28-digit precision, 96-bit mantissa, no NaN/Inf), with the `3.14dec`
> literal suffix, exact arithmetic (`0.1dec + 0.2dec = 0.3dec`), and
> mixed-type promotion where Float wins on mix (`decimal + float → float`).
> This document reflects the v0.7 execution model; `architecture.md` covers
> the compiler/VM/dispatch/path/object/closure/module internals.
>
> **Feature-Parity Round-Out (v0.10):** closes the remaining native-surface
> gaps flagged by the post-v0.8 audit. M130 adds `map-each`/`remove-each`/
> `collect`+`keep` (dynamic-scope accumulator on `Env.collect_stack`) and
> codec natives (`checksum`/`compress`/`decompress`/`enbase`/`debase`/
> `encode`/`decode`). M131 adds object/context reflection (`set?`/`bound?`/
> `bind?`/`context-of`/`context?`/`spec-of`/`body-of`/`resolve`/`has`/
> `extend`) plus `protect`/`unprotect`/`protect-system` enforced at every
> mutating native. M133 adds `floor`/`ceiling`/`truncate`/`zero?`/`positive?`/
> `negative?`/`sign-of`/`gcd`/`lcm`/`sinh`/`cosh`/`tanh`/`square-root`/
> `absolute`. M134 adds `dump` + `errors`s catalog. M135 adds `exports-of`.
> M136 widens `find`/`append`/`copy`/`replace`/`round`/`parse` refinements.
> **Dropped after Red-parity confirmation:** M132 (`quote`/`meta`/`uneval`/
> `eval-set`) and M134's `stop?` — audit misidentifications of Red primitives
> that don't exist in the target parity version. **Demoted to v0.11:** the
> `math` evaluation-order mode and the user-level `trace` toggle (both need
> eval-loop hooks that break the v0.10 "additive native only" non-goal);
> `append/line` (needs `Series` per-element line-hint metadata). See
> `plan13-feature-parity.md`.
>
> **Execution model (v0.3+, unchanged in v0.7):**
> - **Bytecode compiler + stack VM** (`EvalMode::Vm`, the default): blocks
>   compile to a flat `Vec<Instr>` with a constant pool; the VM dispatches
>   instrs with lexical addressing (`LoadLocal(depth, slot)` /
>   `LoadGlobal(slot)`) where statically analyzable, falling back to the
>   dynamic `Context` slot mechanism (`LoadDynamic(sym)`) for `bind`/`use`/
>   `do`-on-data. Tail-call optimization (`TailCall`/`TailReenter`) bounds
>   call-stack depth for tail-recursive programs. Compiled blocks are cached
>   per-`FuncDef` and per-`Series` identity. v0.5 adds `MakeClosure`/
>   `LoadCapture`/`SetCapture` instrs for closure capture cells.
> - **Tree-walking evaluator** (`interp_walker.rs`, the v0.2 default): retained
>   as the `--walk` fallback and the path for `needs_rebind`-flagged blocks.
>   `--features force-walk` runs the entire test suite against the walker for
>   byte-for-byte parity with the VM.
> - **CLI flags:** `--walk` (force tree-walker), `--disasm <file.red>` (print
>   bytecode disassembly, no run), `--disasm-func <name> <file.red>`
>   (disassemble a named func body), `--trace` (per-instr VM trace to stderr),
>   `--module-path <dir>` (search dir for `import %file`, repeatable),
>   `--no-stdlib` (skip stdlib auto-import), `--allow-shell` (gates
>   `call`/`shell`), `--allow-network` (gates `read url!`/`open url!`/
>   HTTP-port reads — default off), `--unset-on-unbound` (v0.7 M86 —
>   evaluate truly-unbound words to `unset!` instead of erroring; default
>   off, back-compat).
> - **Performance:** 2–4× speedup over the walker on compute-heavy programs
>   (deep recursion, tight loops). See `BENCHMARKS.md`.

## Goals
- Lexer → parser → tree-walking evaluator for a small Red subset
- `Red []` header convention, stdout-only I/O
- Cargo workspace with multiple crates
- Integration tests + golden files for parser/printer round-trips and program execution

## Workspace layout

```
rebol-clone/
├── Cargo.toml                    # [workspace] manifest, members only
├── crates/
│   ├── red-core/                 # Value model, lexer, parser, printer, error/source
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── value.rs          # Value enum + Span/Symbol/Series/Binding/FuncDef/ErrorValue/ObjectDef
│   │   │   ├── context.rs        # Context (ordered Symbol→slot + Vec<RefCell<Value>>)
│   │   │   ├── env.rs            # Env, CallFrame, EvalError, NativeFn, RefineArgs
│   │   │   ├── error.rs          # unified Error enum + render_error
│   │   │   ├── source.rs         # LineMap (byte-offset → line:col)
│   │   │   ├── lexer.rs          # Source -> tokens (curly/bracket strings, comments, numbers, words, files, urls)
│   │   │   ├── parser.rs         # Tokens -> Value tree (with source spans); path assembly
│   │   │   └── printer.rs        # Value -> Red source text (mold + form)
│   │   └── tests/
│   │       ├── round_trip.rs     # golden: load -> mold == normalized source
│   │       ├── property.rs       # proptest printer/parser round-trip
│   │       ├── common/mod.rs     # fixture walker
│   │       └── golden/           # *.red + *.expected
│   │
│   ├── red-eval/                 # Tree-walking interpreter
│   │   ├── Cargo.toml            # depends on red-core; ureq for url reads
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── context.rs        # 9-line `pub use` re-export of Env/Context/... from red-core
│   │   │   ├── interp.rs         # dispatch shim: eval(Value, &mut Env) — routes to walker or VM by env.mode
│   │   │   ├── interp_runner.rs  # run_source*/run_series*/RunOptions entry points (extracted in M36)
│   │   │   ├── interp_walker.rs  # tree-walking evaluator (the eval algorithm; entry points moved to interp_runner.rs)
│   │   │   ├── natives/          # native ops split by concern: io/compare/control/func/eval/words/registry
│   │   │   ├── series.rs         # first/next/append/select/find/... series natives
│   │   │   ├── binding.rs        # bind_pass / bind_function_body + bind/use/in/get/set/value? natives
│   │   │   ├── parse.rs          # parse dialect (matcher subset)
│   │   │   ├── strings.rs        # rejoin/reform/join/split/trim/replace/uppercase/lowercase/suffix?
│   │   │   ├── math.rs           # + - * / infix + abs/negate/min/max/round/random/power/and/or/xor/complement/shift-*/even?/odd?/prefix aliases
│   │   │   ├── convert.rs        # to-* family + make/to/form
│   │   │   ├── object.rs         # make object! + object?/same?/words-of/values-of/reflect/in/object/context
│   │   │   ├── path.rs           # path?/get-path?/lit-path?/to-path/to-get-path/to-lit-path
│   │   │   └── io.rs             # read/write/save/load/exists?/size?/modified?/dir?/make-dir/delete/rename/change-dir/what-dir/get-env/set-env/env/wait/call/shell
│   │   └── tests/
│   │       ├── programs.rs       # run .red file, compare stdout to .expected
│   │       ├── programs_errors.rs # run .red file, assert rendered error contains substring
│   │       ├── common/mod.rs     # fixture walker + BufferWriter
│   │       ├── programs/         # *.red + *.expected (golden programs)
│   │       ├── programs_errors/  # *.red + *.expected (golden errors)
│   │       └── fixtures/         # committed file fixtures for io.rs read tests
│   │
│   └── red-cli/                  # Binary entry point
│       ├── Cargo.toml            # depends on red-eval; rustyline for REPL
│       ├── src/
│       │   ├── main.rs           # `red [--allow-shell] path/to/file.red` and `red` (REPL)
│       │   └── repl.rs           # rustyline REPL with multi-line input
│       └── tests/
│           └── cli.rs            # assert_cmd against fixtures
│
└── examples/                     # Sample .red programs usable from CLI
```

## Value model (`red-core/src/value.rs`)

A single `Value` enum backed by shared storage + cursors for blocks (full
series semantics), and by `Rc<FuncDef>` for function values:

```rust
struct Series {
    data: Rc<RefCell<Vec<Value>>>,
    index: usize,            // 0..=len; cursor for series natives
}

enum Binding {
    Unbound,
    Local(Rc<Context>, usize),   // shared context + slot index
    Func(usize),                 // function-local slot index (resolved via call frame)
    Lexical(usize, usize),       // VM-only: (frame-depth, slot) — resolved via the VM frame stack
    Closure(usize),              // M60: index into the closure's capture cell (`ClosureDef.captures`)
}

struct FuncDef {
    params: Vec<Symbol>,
    refinements: Vec<(Symbol, Vec<Symbol>)>,  // (refinement word, its arg words) — M13
    locals: Vec<Symbol>,                      // explicit <local> words for `function` — M16
    param_types: Vec<Option<Rc<TypesetDef>>>,  // M89 per-param runtime type-check; None = unchecked
    body: Series,
    ctx: Context,            // definition context (parent for lookups)
    native: Option<NativeFn>,
    variadic: bool,
    infix: bool,
}

type NativeFn = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

enum Value {
    None,
    Unset,                                     // unset! — M86 (synthetic, no span)
    Logic(bool),
    Integer { n: i64, span: Span },
    Float { f: f64, span: Span },
    Percent { value: f64, span: Span },        // 50% — M80 (stored fractional)
    Money { amount: Rc<MoneyValue>, span: Span }, // $10.00 — M80 (int cents + ccy)
    Issue { s: Rc<str>, span: Span },           // #ABC — M80
    Email { addr: Rc<str>, span: Span },        // foo@bar.com — M80
    Tag { text: Rc<str>, span: Span },          // <b> — M81
    String { s: Rc<str>, span: Span },       // {"..."} and "..." alike
    Word { sym: Symbol, binding: Binding, span: Span },    // foo
    SetWord { sym: Symbol, binding: Binding, span: Span }, // foo:
    GetWord { sym: Symbol, binding: Binding, span: Span }, // :foo
    LitWord { sym: Symbol, span: Span },     // 'foo
    Block { series: Series, span: Span },    // [...] — code is data
    Paren { series: Series, span: Span },    // (...)
    Func(Rc<FuncDef>),                        // synthetic — no span; `type?` → native!/op!/function! (M87)
    Path { parts: Vec<Value>, span: Span },              // foo/bar      — M19
    GetPath { parts: Vec<Value>, span: Span },           // :foo/bar     — M19
    LitPath { parts: Vec<Value>, span: Span },           // 'foo/bar     — M19
    SetPath { parts: Vec<Value>, span: Span },           // obj/field:   — M19
    Refinement { sym: Symbol, span: Span },              // /foo         — M13
    File { path: Rc<str>, span: Span },                  // %foo/bar     — M20
    Url { url: Rc<str>, span: Span },                    // http://…     — M20
    String8 { bytes: Vec<u8>, span: Span },              // binary! #{hex} — M41 (was a stub)
    Error(Rc<ErrorValue>),                   // caught error value — M16 (M42: full field set)
    Object(Rc<RefCell<ObjectDef>>),          // make object! — M18 (synthetic, no span)
    Char { c: char, span: Span },            // #"a" / #"^-" / #"^(41)" — M38
    Map(Rc<RefCell<MapDef>>),                // make map! — M43 (synthetic, no span)
    Pair { x: Rc<Value>, y: Rc<Value>, span: Span },     // 100x200     — M44
    Tuple { bytes: Rc<[u8]>, span: Span },   // 255.0.0 / 128.64.32.128 — M44
    Date { dt: Rc<DateValue>, span: Span },  // 29-Jun-2024/12:30:00+5:30 — M45
    Bitset(Rc<RefCell<BitsetDef>>),          // charset "ABC" — M46 (synthetic, no span)
    Closure(Rc<ClosureDef>),                 // closure! — M60 (synthetic, no span)
    Module(Rc<RefCell<ModuleDef>>),          // module! — M61 (synthetic, no span)
    Hash(Rc<RefCell<HashDef>>),             // hash! — M83 (synthetic, no span)
    Vector(Rc<RefCell<VectorDef>>),          // vector! — M84 (synthetic, no span)
    Image(Rc<RefCell<ImageDef>>),            // image! — M85 (synthetic, no span)
   Typeset(Rc<TypesetDef>),                // typeset! — M89 (synthetic, no span)
}

// v0.5 (M60): closure! — snapshot-capture first-class function.
struct ClosureDef {
    func: Rc<FuncDef>,                        // the underlying FuncDef (spec/body/ctx)
    captures: Rc<Vec<RefCell<Value>>>,        // freevar values, indexed by `freevars` order
}

// v0.5 (M61): module! — self-contained namespace with exported words.
struct ModuleDef {
    ctx: Rc<Context>,                        // the module's namespace
    exports: RefCell<HashSet<Symbol>>,       // words marked `export`
    name: Option<Symbol>,                    // for named modules (`module 'foo [...]`)
    source: Option<Rc<str>>,                 // canonical path for caching (M62)
    parent: Option<Rc<Context>>,             // script user_ctx or another module (reserved v0.6+)
}

struct ObjectDef {
    ctx: Rc<Context>,
    parent: Option<Rc<RefCell<ObjectDef>>>,
    self_word: Symbol,
}

// v0.4 (M43): map! — insertion-ordered heterogeneous key→value table.
enum MapKey { Sym(Symbol), Int(i64), Str(Rc<str>), Char(char), Bool(bool), None }
struct MapDef { entries: RefCell<IndexMap<MapKey, Value>> }   // indexmap dep

// v0.4 (M45): date!/time! — single variant covers date-only / date+time /
// date+time+zone. Timezone model: fixed UTC offsets only (minutes east of
// UTC); None = zone-naive. No named zones, no DST (matches Red parity).
struct DateValue { dt: NaiveDateTime, zone: Option<i32> }

// v0.4 (M46): bitset! — bit-packed byte set for parse charset matching.
struct BitsetDef { bits: RefCell<Vec<u64>>, len: usize }

// v0.7 (M80): money! — fixed-point currency (integer cents + 3-letter code).
struct MoneyValue { cents: i64, currency: Rc<str> }   // default "USD"

// v0.7 (M83): hash! — unordered key→value table (HashMap-backed). A
// `series!` (unlike `map!`): indexable/sliceable as alternating key/value
// pairs. `keys-of`/mold use a side `key_order` vec for test determinism
// (documented deviation from Red's unspecified iteration order).
struct HashDef {
    entries: RefCell<HashMap<MapKey, Value>>,
    key_order: RefCell<Vec<MapKey>>,
}

// v0.7 (M84): vector! — typed numeric series (i8/i16/i32/i64/f32/f64).
// `kind` drives narrow-on-write; `elems` is `Vec<Value>` for native-compat
// (packed-array wording aspirational; perf deferred to v0.8). `cursor`
// mirrors Red's series cursor for `next`/`back`/`at`/`skip`/`head`/`tail`/
// `index?` (return positioned `Block` views via `extract_series` —
// documented deviation from Red).
struct VectorDef {
    kind: RefCell<Symbol>,            // 'integer!/'float!/...
    elems: RefCell<Vec<Value>>,
    cursor: RefCell<usize>,
}

// v0.7 (M85): image! — fixed-size 2D RGBA8 pixel buffer (row-major).
// NOT a `series!`: only `length?`/`pick`/`poke` apply (size is fixed).
struct ImageDef {
    width: usize,
    height: usize,
    pixels: RefCell<Vec<[u8; 4]>>,   // RGBA8
}

// v0.7 (M89): typeset! — set of type-word symbols for runtime type-checking.
struct TypesetDef { types: RefCell<HashSet<Symbol>> }   // 'integer!/'float!/'any-word!/...
```

### Type Completeness (v0.7)

v0.7 (M80–M89, `plan8-missing-types.md`) closes the remaining type-gap
between the POC's `Value` enum and the Red/Rebol value type inventory by
landing **nine new value variants** plus two type-layer refinements.
`regex!`/`struct!`/`handle!` were deferred to v0.8 (see "Known gap"
below). The new types are **all additive through the existing
`Const`-pool + native-call path** — no new `Instr` variants, no new
hot-path work. New `Value` variants:

- **M80 — `percent!`/`money!`/`issue!`/`email!`:** four source-origin
  scalar literals with single lexer rules and trivial mold/form.
  `percent!` is `Float`-backed (stored fractional: `50%` ⇒ 0.5); `money!`
  is fixed-point (integer cents + 3-letter currency code, default `"USD"`);
  `issue!` is a `Rc<str>` body (`#ABC`); `email!` is a `Rc<str>` address.
- **M81 — `tag!`:** HTML/XML-style tag literal (`<b>`, `<img src="x">`).
  Standalone lexer rule on `<` (no collision with the `<`/`<=`/`<>`
  comparison operators, which lex as `Word` tokens — `<` starts a tag
  only when followed by a non-delimiter, non-operator char).
- **M83 — `hash!`:** an unordered `HashMap`-backed key→value table,
  distinct from `map!` (insertion-ordered `IndexMap`) in two ways: (1)
  iteration order is unspecified (Red parity — `hash!` is the perf table,
  `map!` is the ordered one); (2) `hash!` IS a `series!` — indexable/
  sliceable as alternating key/value pairs, `foreach`-able. `keys-of`/
  mold use a side `key_order` vec for test determinism only (documented
  deviation).
- **M84 — `vector!`:** a typed numeric series (`integer!`/`float!` element
  kind, narrow-on-write). Stored as `Vec<Value>` for native-compat
  (packed-array wording aspirational; perf deferred to v0.8). Full
  `series!` model: `length?`/`pick`/`poke` (1-based, narrows to kind),
  cursored nav returns positioned `Block` views via `extract_series`
  (documented deviation from Red).
- **M85 — `image!`:** a fixed-size 2D RGBA8 pixel buffer (row-major). NOT
  a `series!` (size is fixed): only `length?` (== `w*h`)/`pick`/`poke`
  apply; `append`/`insert`/etc. error. Path access: `image/width`/
  `image/height` → integer; `image/size` → pair; `image/<n>` → flat
  pixel pick (1-based); `image/<x>x<y>` → coord pick via `Word("/") +
  Pair` parser folding. No GUI/draw — pure data.
- **M86 — `unset!`:** a distinct "no value" sentinel, separate from
  `none!`. Synthetic (no span); molds/forms to the empty string. The one
  v0.7 behavior change is **gated and opt-in** (default off): the
  `--unset-on-unbound` CLI flag + `Env::unset_on_unbound: bool` make a
  truly-unbound word evaluate to `Value::Unset` instead of erroring
  (mirrors Red). With the flag off (default), all existing unbound-word
  error fixtures stay green.
- **M89 — `typeset!`:** a value representing a set of type-word symbols
  (`'integer!`/`'float!`/`'any-word!`/…). Used in function spec blocks
  for runtime type-checking: `func [x [integer! float!]]` builds a
  `Rc<TypesetDef>` and stores it in a new `FuncDef.param_types:
  Vec<Option<Rc<TypesetDef>>>` parallel to `params` (None = unchecked,
  back-compat). Both walker and VM call paths check
  `param_types[i].accepts(arg)` before binding; on failure they raise
  `EvalError::Native` with a `"type error: arg N expected [ts], got
  <found>"` message. The `any-*` family (`any-word!`/`any-path!`/
  `any-string!`/`any-block!`/`any-object!`/`any-function!`/`number!`/
  `series!`/`any-type!`) is recognized by `TypesetDef::accepts` via a
  group-word table. The typeset *algebra* (`union`/`intersect`/
  `complement`) is deferred to v0.8.

Two type-layer refinements (no new `Value` variants):

- **M87 — `native!`/`op!` split:** `Value::Func` keeps its
  `FuncDef.native`/`FuncDef.infix` flags (avoiding a sweeping match-arm
  refactor), but `type?`/`native?`/`op?`/`any-function?`/`types-of` report
  them as distinct types: `+` is `op!` (not `native!`), `print` is
  `native!`, a user `func` is `function!`, a `closure` is `closure!`.
  `native?` and `op?` are disjoint; `function?`/`any-function?` cover
  all four.
- **M86 `unset!` gate** (see above) — the only v0.7 behavior change,
  default off.

`Symbol` = an `Rc<str>` newtype (the `string_cache` crate was tried early on
but dropped in favor of the simpler `Rc<str>`; no profiling need surfaced).

`Context` is defined in `red-core/src/context.rs` (an ordered
`Symbol -> slot index` map plus a `Vec<RefCell<Value>>` of slots). Note the
split: `context.rs` holds **only** `Context`; `Binding`, `FuncDef`,
`Symbol`, `Series`, `Value`, `ErrorValue`, `ObjectDef`, `MapDef`, `MapKey`,
`BitsetDef`, and `DateValue` all live in `value.rs`. `Env`, `CallFrame`,
`EvalError`, `NativeFn`, and `RefineArgs` live in `red-core/src/env.rs` (so
red-core's printer/parser can mention `EvalError` without a red-eval
dependency). `red-eval/src/context.rs` is a 9-line `pub use` re-export of all
those names.

`red-core` depends on `indexmap` (for `MapDef`'s insertion-ordered map, M43)
and `chrono` (for `DateValue` / `now` / timezone offsets, M45) — the first
non-std deps in `red-core` (zero-dep was never a documented design goal;
`red-eval` already pulled `ureq`).

## Red blocks — semantics notes

Blocks (`[...]`) are the central data structure in Red: **code is data**. The
POC implements the full series model *and* word binding.

- **Homoiconicity**: a block is a `Vec<Value>`; evaluating it walks values in
  order. The same block is usable as data (molded, indexed, sliced) and as
  code (via `do` / `reduce` / top-level script `do`). (`compose` is **not**
  implemented — deferred to v0.3.)

- **Evaluation rule**:
  - A `Block` value encountered by `eval` is returned **as-is** (data).
    Only `do`, `reduce`, and the top-level script loader walk into a block.
  - A `Paren` value encountered by `eval` is evaluated **eagerly** in place
    (like an inline `do`). This distinction is load-bearing.

- **Argument convention**: a block passed as an argument is *not* evaluated
  by the caller; the callee decides. `if`/`either`/`loop`/`until`/`repeat`
  receive a block and `do` it themselves; `print`/`+`/etc. receive already-
  evaluated values.

### Series model (full)

A block is `Series { data: Rc<RefCell<Vec<Value>>>, index: usize }` so the
same underlying storage can be shared by multiple positioned views (Red's
`series!` semantics).

- Type predicates: `series?`, `block?`, `paren?`, `any-block?`, `empty?`.
- Navigation: `first`, `second`, `third`, `last`, `next`, `back`, `at`,
  `skip`, `head`, `tail`, `index?`, `length?`.
- Access: `pick`, `poke`, `select`, `find` (linear; no `match`/regex in POC).
- Mutation: `append`, `insert`, `change`, `remove`, `clear`, `take`, `poke`.
- Slicing: `copy/part`, `at`-based sub-series share storage (copy-on-write
  deferred — note as future work).
- Iteration: `foreach` (over block or series), `repeat`, `forall` (uses the
  series cursor), `while`/`until`.
- `series/head`, `series/index`, etc. paths are out of scope; use natives.

Series natives operate on the cursor; `next` returns a new `Series` pointing
one ahead; mutation via `append`/`insert` affects shared storage (matches
Red's reference semantics).

### Binding & contexts (real implementation)

Words inside blocks are **bound** to contexts. The POC implements Red-style
binding, not just dynamic lookup.

- `Context` = an ordered map of `Symbol -> slot index` plus a
  `Vec<RefCell<Value>>` of slots. Self-referential (a context can hold a
  value that references itself) via `Rc<RefCell<...>>`.
- `Word` carries a `Binding`: `Unbound`, `Local(Rc<Context>, slot)`,
  `Func(param_index)` (resolved via the active call frame), `Lexical(depth,
  slot)` (VM-only, resolved via the VM frame stack), or `Closure(idx)`
  (index into the closure's capture cell). Binding is attached by the
  **binding pass** in `red-eval/src/binding.rs` (run before eval, not inside
  the parser) for script-level words, and at `make`/`func`/`function`/
  `make object!`-creation time for function bodies.
- `set-word` in a script binds into the **user context** (a single top-level
  context for the POC, held as `Rc<Context>` on `Env`).
- `func` / `does` / `function` / `make function!` create function values
  with their own context (parent = definition context). `func` uses shallow
  copy of args on each call.
- Lookup walks: word's binding → if `Local`/`Func`/`Lexical`/`Closure`,
  read the corresponding slot/capture; if `Unbound`, consult `user_ctx`
  first (so a later `import`/`set` that populated `user_ctx` resolves),
  then the native registry, then error (Red-style "has no value"). The
  `Unbound → user_ctx` fallback is the one v0.5 behavior change (M62),
  required so `import`-aliased words resolve without AST re-walking.
- `bind`, `use`, `in`, `value?`, `get`, `set` natives to manipulate bindings
  explicitly. (`in` is registered in `object.rs`.)
- **Objects** implemented in v0.2 (M18): `Value::Object`, `make object!`/
  `object`/`context`, prototype inheritance (copy-based), `in`, `self`
  reference, `words-of`/`values-of`/`reflect`, predicates `object?`/
  `same?`/`not-same?`.
- **Closures & Modules (v0.5):**
  - `closure!` (`Value::Closure`): first-class closures with **snapshot
    freevar capture** — `closure [spec][body]` copies each freevar's value
    into an owned `Vec<RefCell<Value>>` cell at creation time. Outer writes
    after closure creation don't propagate inward; inner writes don't
    propagate outward (the `RefCell` permits mutation across invocations of
    the *same* closure, but two closures closing over the same outer `x` get
    independent cells). This fixes the v0.3 escaping-closure bug
    (frame-chain-walking returned stale values once a `func` escaped its
    defining frame). `func`/`does`/`function` keep their shallow-copy
    semantics (back-compat with v0.2–v0.4 golden fixtures). **Deviation from
    Red:** real Red `closure!` shares the cell across closures and across
    outer/inner (inner writes propagate outward); shared-cell is a v0.6
    candidate. SetWord inside a closure body is treated as a local (not a
    capture write) — use block-as-state (`poke`) for mutable closure state.
  - `module!` (`Value::Module`): a self-contained namespace (`ModuleDef.ctx`)
    with a set of exported words. `module [body]` evaluates the body with
    `env.user_ctx` swapped to the module's ctx (mirrors `make object!`);
    `export 'word` / `export [words]` marks words for public visibility.
    `module 'name [body]` is cached on `Env::modules` (singleton by name).
    `import 'name` / `import %file.red` / `import <module-value>` aliases a
    module's exported words into the current `user_ctx` (overwriting
    existing slots). File imports are cached by canonical path on
    `Env::modules_by_path`; circular imports are detected and raise an
    error. Visibility: inside the module body all words are visible;
    `module/word` from outside resolves only into `exports` (private →
    `UnboundWord`). Path resolution mirrors `object/field`.
  - CLI: `--module-path <dir>` (repeatable, populates
    `system/options/module-path`) and `--no-stdlib` (skip stdlib auto-import).
    The stdlib (~25 utility functions: string/block/math utils + a pure-Red
    `sort`) is auto-imported into `user_ctx` at script start unless
    `--no-stdlib` is set.
- Known gap (v0.5): shared-cell closures (proper SetWord capture) and
  `unimport` are v0.6 candidates. Named timezones (`chrono-tz`) are
  deferred to v0.6+. `DD/MM/YYYY` is not supported (`/`
  is a lexer delimiter — use `DD-Mon-YYYY` or `YYYY-MM-DD`). `pair!`/`tuple!`
  `same?` returns `false` (immutable value types; use `=`` for structural
  equality). The structured error model
  (`code`/`type`/`args`/`near`/`where`/`by`) IS in v0.4 (M42). Block-integer
  SetPath (`b/2: 99`) works (M38 follow-up). See `plan6-closures-modules.md`.
- Known gap (v0.7): the new types landed (M80–M89), but the following
  remain deferred to v0.8 — `regex!`/`struct!`/`handle!` (M82/M88 were
  skipped; they land alongside the `routine!` FFI binding layer in v0.8),
  `ref!` (internal C-level reference type, no script surface — closest
  POC equivalent is the deferred `handle!`), advanced `bitset!`/`logic!`
  ops beyond membership, `object!` `on-change` reactive slots,
  `typeset!` algebra (`union`/`intersect`/`complement`), the full `port!`
  model backed by `Channel` (concurrency), and shared-cell closures
  (proper SetWord capture). The `--unset-on-unbound` fallback is default
  off (opt-in); revisit the default in v0.8. See `plan8-missing-types.md`.
- Known gap (v0.10): the speculative M132 primitives (`quote`/`meta`/
  `uneval`/`eval-set`) and M134's `stop?` were dropped after Red-parity
  confirmation — they don't exist in the target parity version. The `math`
  evaluation-order mode and the user-level `trace` toggle are demoted to
  v0.11 (both require eval-loop hooks that break the "additive native only"
  v0.10 non-goal). `append/line` deferred (needs `Series` per-element
  line-hint metadata). `find/only`/`/any`/`/with`/`/skip` refinements not
  yet implemented (M136 landed `/part`/`/last`/`/tail`/`/match`). `checksum`
  exposes CRC32 + SHA-256 only (`'sha1` errors — `sha2` crate doesn't
  include it). See `plan13-feature-parity.md`.
- Known gap (v0.6): the `port!`/networking surface is a **synchronous,
  GET-only subset** — `read http://`/`read https://` (via `ureq`, TLS on by
  default) and `open`/`close`/`create`/`read port`/`write port` for files.
  Non-HTTP protocols (FTP/SMTP/POP3/NNTP/DNS/TCP/UDP/WHOIS/Finger/Daytime)
  are reserved as `PortScheme` variants that error in v0.6 (they return
  `NetError::UnsupportedInV09`); HTTP methods beyond GET, request headers/
  cookies/auth, redirect control, `write http://` (POST/PUT), and the
  async/`Channel`-backed port model are deferred to v0.7+. Network access
  is gated behind `--allow-network` (default off, mirroring `--allow-shell`).
  See `plan11-functional-gaps.md` and
  `docs/notes/rust-networking-protocol-crate-recommendation.md`.

### Spans
Each `Block`/`Paren` retains the span of its `[...]`/`(...)` delimiters;
inner values already carry their own spans. Required for `do`-time errors and
for `bind` to report unbound words with a location.

### Built-ins (full block set)
- Type predicates: `block?`, `paren?`, `series?`, `any-block?`, `empty?`,
  `object?`, `same?`, `not-same?`, `file?`, `url?`, `function?`,
  `path?`, `get-path?`, `lit-path?`. v0.7 additions: `percent?`,
  `money?`, `issue?`, `email?`, `tag?`, `unset?`, `hash?`, `vector?`,
  `image?`, `typeset?`, `native?`, `op?`, `any-function?` (M87:
  `native?`/`op?` disjoint; `function?`/`any-function?` cover all four
  of `function!`/`native!`/`op!`/`closure!`).
- Series nav: `first` `second` `third` `last` `next` `back` `at` `skip`
  `head` `tail` `index?` `length?`.
- Series access: `pick` `poke` `select` `find` (with `/case` refinement).
- Series mutate: `append` (`/only`) `insert` `change` `remove` `clear` `take`
  `copy` (`/part`). `sort` (`/case`/`/reverse`/`/skip size`/`/compare func`,
  native — shadows the stdlib version). Series set ops: `unique`
  (`/case`/`/skip`), `intersect`/`union`/`difference`/`exclude`
  (`/case`/`/skip`) on `block!`/`string!` (the same names dispatch on
  `bitset!` operands to the M46 implementation).
- Iteration: `foreach` `forall` `forskip` `while` `until` (plus `loop`/
  `repeat`/`forever`/`for`).
- Binding: `bind` `use` `in` `value?` `get` `set`.
- Functions: `func` `does` `function` `make` `function?` `return` `exit`
  `quit`.
- Control flow: `if` `unless` `either` `loop` `repeat` `until` `while`
  `forever` `for` `do` `reduce` `break` `continue` `switch`
  (`/default` `/case`) `case` (`/default` `/all`) `default` `all` `any`
  `try` `attempt` `catch` `throw` `cause-error` `comment` `exit`/`quit`.
- Arithmetic (infix + prefix): `+` `-` `*` `/` `//` (modulo) `**` (power)
  `add` `subtract` `multiply` `divide` `abs` `negate` `min` `max` `round`
  (`/to` `/even`) `random` (`/seed` `/only` `/secure`) `power`.
- Comparison: `=` `<>` `<` `>` `<=` `>=`.
- Logic / bitwise: `and` `or` `not` `xor` `complement` `shift-left`
  `shift-right` `even?` `odd?`.
- Eval: `do` `reduce` `load` (string→block; file/url-aware override in `io.rs`).
  `mold` (`/only`) — callable native wrapping the printer (v0.6).
- Strings: `rejoin` `reform` `join` `suffix?` `split` (`/with`) `trim`
  (`/auto` `/with` `/lines` `/all`) `replace` (`/all`) `uppercase`
  (`/part`) `lowercase` (`/part`).
- Conversions: `to-integer` `to-float` `to-string` `to-block` `to-word`
  `to-set-word` `to-get-word` `to-lit-word` `to-logic` `to-file` `to-url`
  `to-path` `to-get-path` `to-lit-path` `make` `to` `form`. v0.7 additions:
  `to-percent`/`to-money`/`to-issue`/`to-email`/`to-tag`/`to-hash`/
  `to-vector`/`to-image`/`to-typeset` (each paired with a `make <type!>`
  constructor; see M80–M89).
- Objects: `make object!` `object` `context` `words-of` `values-of`
  `reflect` `in`.
- File / shell I/O (M20): `read` (`/lines` `/binary`) `write` (`/append`
  `/lines` `/binary`) `save` `load` `exists?` `size?` `modified?` `dir?`
  `make-dir` `delete` `rename` `change-dir` `what-dir` `get-env` `set-env`
  `env` `wait` `call` `shell` (the last two gated on `--allow-shell`).
- Ports & networking (v0.6, M113): `open` (`file!`/`url!`), `close`,
  `create` (`file!`), `port?`, `read port` (streaming for HTTP, whole-file
  for files), `write port`. `read url!` for `http://`/`https://` routes
  through the `net/` facade (GET-only). All network access gated on
  `--allow-network` (default off, mirroring `--allow-shell`).
- Constants: `none` `true` `false` `newline` `system` (object exposing
  `system/options/{args, allow-shell, allow-network, path, module-path}`).
- Closures & modules (v0.5): `closure` `closure?` `module` `module?` `export`
  `import`.
- Implemented in v0.2 (M13–M20): refinements (`/part`, `/case`, … as a
  general dispatch mechanism), real paths (`obj/field`, `block/2`,
  `set-path`), `Object`/`make object!`, `File`/`Url` literals + the I/O
  surface above, the type-conversion and string/math surfaces above.
  v0.4 (M38–M46): `char!`/`binary!`/`map!`/`pair!`/`tuple!`/`date!`/
  `bitset!`, `compose`, trig math, the full `error!` model, the completed
  `parse` dialect. v0.5 (M60–M65): `closure!` (snapshot capture),
  `module!`/`export`/`import`, the stdlib, `--module-path`/`--no-stdlib`.
  v0.5.1 (M120–M121): **control-flow completeness** — `unless`, `forever`,
  `for` (counted, direction-aware, int/float/char), `forskip` (record-wise
  series iteration). See `plan12-control-flow.md`.
  v0.6 (M110–M114): **core functional gaps** — `parse` named-rule recursion
  (a bound word resolving to a `block!` is treated as a sub-rule, with a
  depth guard); `mold` exposed as a callable native (`/only` refinement);
  series `sort` (native, shadowing the stdlib version) + set operations
  `unique`/`intersect`/`union`/`difference`/`exclude` on `block!`/`string!`;
  `port!` value type + minimal synchronous networking (`open`/`close`/
  `create`/`read port`/`read url!` for HTTP/HTTPS GET via the existing
  `ureq` dep — TLS on by default in ureq 2.x, no new dependency) behind a
  `--allow-network` capability gate. See `plan11-functional-gaps.md` and
  `docs/notes/rust-networking-protocol-crate-recommendation.md` (the composed-facade
  rationale).
  v0.7 (M80–M89): **type completeness** — `percent!`/`money!`/`issue!`/
  `email!`/`tag!`/`unset!`/`hash!`/`vector!`/`image!`/`typeset!`,
  `native!`/`op!`/`any-function?` type predicates, `--unset-on-unbound`
  gate (default off), `typeset!`-backed typed-func arg type-checking.
  `regex!`/`struct!`/`handle!` deferred to v0.8. See
  `plan8-missing-types.md`.
  v0.10 (M130–M136): **feature-parity round-out** — series/string DSL
  (`map-each`/`remove-each`/`collect`/`keep`); codec (`checksum`/`compress`/
  `decompress`/`enbase`/`debase`/`encode`/`decode`); object/context
  reflection (`set?`/`bound?`/`bind?`/`context-of`/`context?`/`spec-of`/
  `body-of`/`resolve`/`has`/`extend`/`protect`/`unprotect`/`protect-system`);
  math helpers (`floor`/`ceiling`/`truncate`/`zero?`/`positive?`/`negative?`/
  `sign-of`/`sign?`/`gcd`/`lcm`/`sinh`/`cosh`/`tanh`/`square-root`/
  `absolute`); eval reflection (`dump`/`errors`); module extras (`exports-of`);
  refinement expansion on `find`/`append`/`copy`/`replace`/`round`/`parse`.
  `quote`/`meta`/`uneval`/`eval-set`/`stop?` dropped (audit
  misidentifications); `math` eval-order mode + user-level `trace` demoted
  to v0.11. See `plan13-feature-parity.md`.
- Optional/deferred: shared-cell closures, `unimport`, reactivity (v0.8);
  concurrency (v0.8); `regex!`/`struct!`/`handle!` (v0.8, alongside
  `routine!` FFI); `ref!` (internal C-level reference, no script surface);
  `routine!` FFI, named timezones, the full port model, `typeset!` algebra.
  `recurse`/`recur` (anonymous self-reference) is deferred to v0.8+ as a
  possible ergonomic extension — not a Red-parity gap. (`parse` is in
  scope — see "Dialects".)

## Dialects

A **dialect** in Red is any block evaluated by a custom interpreter instead
of the default `do` evaluator. Blocks are data, so any native can walk a
block with its own rules. The POC implements one concrete dialect (`parse`);
no typed framework — a dialect is simply a native that interprets a block.

### Dialect concept
- A dialect is just a function `fn(&[Value], &mut Env) -> Result<Value, EvalError>`
  taking a block's contents and interpreting them however it likes.
- `parse` is the only built-in dialect in the POC.
- User-defined dialects are possible by passing a block to a native that
  interprets it (e.g. a future `draw` dialect); no special syntax needed.
- Contrast: `do` is the "Red dialect" (normal eval); `reduce` is the
  "reduce dialect" (eval each value, collect results). (`compose` would be a
  single native eval-parens-leave-rest, but it is **not** implemented in
  this POC — deferred to v0.3.)

### `parse` dialect (in scope for POC)
Mini-DSL on blocks/strings, implemented as a native that walks its rule
block. Works on **both string! and block! input**.

```red
parse "abc" ["a" "b" "c"]          ; => true
parse [1 2 3] [1 2 3]              ; => true
parse "hello world" [copy name to " " skip copy rest to end]
```

POC rule set (matcher subset + v0.4 completions):
 - Literal values (string/integer/word) — match against input.
 - `skip`, `to`, `thru`, `end`, `none`.
 - `any`, `some`, `opt`, `while`.
 - `|` (alternative).
 - `copy word rule` (capture sub-match), `set word rule` (single value).
 - `[...]` grouping (sub-rules).
 - `(...)` (Red code side-effect, evaluated via `eval`).
 - **Named-rule recursion (v0.6, M110):** a bound word that resolves to a
   `block!` is treated as a named sub-rule and parsed recursively against
   the same input cursor (a word resolving to a `bitset!` still does
   charset matching; anything else is a literal-value match). A depth
   guard (`MAX_PARSE_DEPTH`) raises `ParseRecursionLimit` on
   self/mutual-reference loops with no base case.
 - Return `logic!` (matched/not).
 - **v0.4 additions (M46):** `bitset!` as a rule (matches any char in set,
   advances 1); `/case` refinement (case-sensitive string matching);
   `collect 'word rule` / `collect into 'word rule` (accumulate matched
   values into a block, bind word); `keep value` / `keep 'word` /
   `keep (expr)` (push into current collect target); `match value` (like
   literal match but returns the matched value); `into 'word rule` (parse
   a sub-series, bind result); `fail` (always fails — opposite of `none`);
   `break` (exit the current `parse` entirely, return true); `if (expr)`
   (succeed iff expr is truthy, no advance); `not rule` (succeeds iff
   sub-rule fails, no advance); `??` (debug — print current input position
   to stderr); `accept value` (succeed immediately, return value);
   `reject` (fail immediately); `ahead rule` (lookahead, no advance);
   `behind rule` (reverse lookahead).

Deferred: rule compilation, BNF-style grammar extraction, error rule blocks,
`gather`. The matcher + v0.4 rule set is complete enough for typical
parser-construction use.

### Other dialects (illustrative, NOT implemented)
- `load` dialect — already the parser; not a runtime dialect.
- `draw` dialect, `vid` dialect (GUI), `secure` dialect — all out of scope.

### Implications for the rest of the plan
- `parse` is a non-trivial native → gets its own milestone and source file
  (`red-eval/src/parse.rs`).
- `parse` depends on the **series model** (cursor-based input scanning for
  both strings and blocks) and on **binding** (for `copy`/`set` to write
  into the user context).
- Dialects motivate keeping `eval`'s public surface small: a dialect only
  needs `&[Value]` + `&mut Env`, never a re-entry into the parser.

## Lexer (`red-core/src/lexer.rs`)
- Whitespace-delimited tokens (Red's defining feature); `,` is also whitespace
- Comments: `;` to EOL
- Strings: `"..."` (backslash escapes only; unknown escapes kept verbatim)
  and `{...}` (multi-line, balanced braces) — both supported
- Integers and floats (both supported from the start)
- Words (incl. `word:`, `:word`, `'word`), refinements (`/word`),
  files (`%foo` or `%"quoted"`), urls (`scheme://…`)
- `/` is a delimiter (so `foo/bar` splits and the parser re-folds into a
  path); bare `/` → division Word; `//` → modulo Word (one token)
- Blocks `[ ]`, parens `( )`
- Header `Red [...]` recognized at parser level
- Each token carries a `Span { start, end }`

## Parser (`red-core/src/parser.rs`)
- Recursive descent over token stream
- `parse_program` -> expects `Red` word + header block + body (rest as a
  flat `Series`); `load` -> bare body; `load_source` -> lex+load in one call
- Returns a body `Series` (not a wrapped `Value::Block`)
- Errors with spans
- **Path assembly** happens inline during `parse_value` — adjacent
  `Refinement` tokens (and `Word("/")`+value pairs) fold into
  `Path`/`GetPath`/`LitPath`/`SetPath`. `SetPath` is detected via span
  overlap between the trailing `SetWord` and the last `Refinement`.
- **Binding** is NOT done by the parser — it's a separate pass in
  `red-eval/src/binding.rs` (`bind_pass`/`bind_pass_into`) run before eval,
  which collects set-words, loop vars, parse capture words, and `use`/`get`/
  `set`/`value?` operands. Function bodies bind at `func`/`does`/`function`/
  `make object!` creation time.

## Printer / `mold` (`red-core/src/printer.rs`)
- Inverse of parser, used by REPL and tests
- Exports `mold`, `mold_to_string`, `form`, `form_to_string`
- Round-trip property: `mold(load_source(s)) == normalize(s)`

## Evaluator (`red-eval/src/interp.rs`)

`pub fn eval(block: &Value, env: &mut Env) -> Result<Value, EvalError>`

`Env` (defined in **`red-core/src/env.rs`**, re-exported by red-eval) holds
the `user_ctx: Rc<Context>`, the call stack of function `CallFrame`s, the
`natives: HashMap<Symbol, Rc<FuncDef>>` registry, an `out: Box<dyn Write>`
sink, `allow_shell: bool`, and `cwd: PathBuf`. It is not a flat `HashMap`.

- Walks the block, evaluating each value in order; last value returned
- Word: resolve its **binding** → read slot from the bound context; error
  Red-style ("has no value") if `Unbound`. No dynamic global fallback.
- SetWord: eval next value, write into its bound context slot (or bind into
  user context if unbound at script top-level)
- Block: returned **as-is** (data). Only `do`/`reduce`/natives that take a
  block arg walk it. (See "Red blocks" section. `compose` is not implemented.)
- Paren: evaluated **eagerly** in place when its enclosing block is walked
- Path: full M19 path resolution — function-headed (refinements),
  object-headed (field/method), data-headed (`block/2`, `string/3`),
  SetPath writes. See `architecture.md` for the caveats (string char pick
  returns integer; block-integer SetPath unreachable from source).

### Built-in natives (POC set)
See the "Red blocks → Built-ins (full block set)" list above for the
complete set. Headline groups: I/O (`print`, `prin`, `probe`), arithmetic
(`+ - * / // **` plus `add`/`subtract`/… prefix aliases), comparison
(`= <> < > <= >=`), logic/bitwise (`and or not xor complement shift-*`),
control flow (`if`, `either`, `switch`, `case`, `all`, `any`, `try`,
`attempt`, `catch`, `throw`, `default`, `cause-error`, `loop`, `repeat`,
`until`, `while`, `foreach`, `forall`, `break`, `continue`, `exit`, `quit`,
`comment`), eval (`do`, `reduce`, `load`), series ops (full set), binding
(`bind`, `use`, `in`, `value?`, `get`, `set`), functions (`func`, `does`,
`function`, `make`, `function?`, `return`), strings, math, conversions,
objects, file/shell I/O, constants (`none`/`true`/`false`/`newline`/
`system`).

Native calls are implemented in Rust directly against `&[Value]` and
`&mut Env`; `func`/`does` bodies are evaluated by recursing into `eval`
with a fresh child context. Entry points `run_source_with_exit` /
`run_source_with_exit_opts` / `run_series_with_exit_opts` drive a full
script (binding pass + eval) and return `(Option<Value>, i32)` so the CLI
can propagate `quit`/`exit` codes; they take a `RunOptions { allow_shell,
args }`.

## Error model
- `EvalError` enum: `UnboundWord`/`TypeError`/`Arity`/`Native` carry a `Span`;
  `Return`/`Break`/`Continue`/`Throw`/`Quit` are control-flow unwinds (no
  span). **v0.4 (M42):** `Raised(Rc<ErrorValue>)` transports first-class
  `error!` values with the full Red field set (`message`/`code`/`type`/
  `args`/`near`/`where`/`by`). `LexError`/`ParseError` also carry spans.
  All three are unified under `Error` (Lex/Parse/Eval), defined in
  `red-core/src/error.rs`.
- `render_error(file, src, err)` produces
  `*** Error: [file:line:col: ]<msg>` using a `LineMap` (in
  `red-core/src/source.rs`) to translate the
  error's byte-offset span into 1-based line/col. The CLI passes the file
  path + source; the REPL passes `None` + the line buffer. **v0.4 (M42):**
  structured errors with a `type` word render as
  `*** Error: [loc: ]<type> error: <message>` (e.g. `math error: ...`).
  The VM and walker auto-enrich `Native` errors with `where`/`near` via
  `enrich_error`.
- Tests assert against the message-body substring (error fixtures) or the
  rendered `*** Error:` line (CLI tests).
- **v0.4 error natives (M42):** `make error!` (from string or block of
  keyword pairs), `to-error`, `cause-error` (1/2/4-arg + block forms),
  `error-type`/`error-code`/`error-args`/`error-near` accessors,
  `attempted?` predicate. `try`/`attempt`/`catch` unwrap structured payloads.

### Sandbox policy (file & shell I/O — M20)
- `call`/`shell` natives are **off by default** and raise `EvalError::Native`
  ("shell disabled") unless the CLI is invoked with `--allow-shell`. No test
  fixture invokes the shell; the one inline test that does is gated on
  `env.allow_shell = true` set directly in Rust.
- `read` of a `url!` performs real network I/O via `ureq` (http/https only).
  Network-dependent tests are marked `#[ignore]` so `cargo test` stays
  hermetic; run with `cargo test -- --ignored`.
- File I/O (`read`/`write`/`save`/`exists?`/etc.) operates on the real
  filesystem relative to `env.cwd` (set from `std::env::current_dir()` and
  mirrored to `system/options/path`; `change-dir` updates both). Write tests
  use the `tempfile` dev-dep for scratch directories; read tests use
  committed fixtures under `crates/red-eval/tests/fixtures/`.
- `read/binary` and `write/binary` are stubbed (error) pending the `binary!`
  type work deferred to v0.3.

## CLI (`red-cli/src/main.rs`)
- `red [--allow-shell] file.red [args...]` — load, parse, do; exit code from
  last value or from `quit`/`exit`. Trailing args are exposed as
  `system/options/args`.
- `red` (no args) — REPL using `rustyline` (`repl.rs`): read line, `load`,
  `do`, `mold` result, print. Multi-line input is accumulated when the
  parser reports `MissingClose`; `quit`/`exit` at a fresh prompt exits;
  Ctrl-C discards partial input, Ctrl-D exits. Non-tty stdin reads plain
  lines without rustyline.
- `--help` / `-h`, `--version` / `-V`, `--allow-shell` (gates `call`/`shell`),
  `--allow-network` (gates `read url!`/`open url!`/HTTP-port reads — default
  off, mirroring `--allow-shell`), `--unset-on-unbound` (v0.7 M86 —
  evaluate truly-unbound words to `unset!` instead of erroring; default
  off, back-compat).
- `--walk` (force tree-walker), `--disasm <file>` (disassemble, no run),
  `--disasm-func <name> <file>` (disassemble a named func), `--trace`
  (per-instr VM trace to stderr).
- `--module-path <dir>` (repeatable; appends to
  `system/options/module-path`, a `block!` of `file!` dirs searched by
  `import %file.red` when the cwd-relative resolution misses).
- `--no-stdlib` (skip stdlib auto-import; stdlib words like `str-upper`
  stay unbound).

## Testing strategy

**Integration + golden:**

1. `red-core/tests/round_trip.rs` — for each `tests/golden/*.red`:
   - Read source, parse, mold, compare to `*.expected` (normalized form). New test files can be added with no code changes.

2. `red-core/tests/property.rs` — `proptest` printer/parser round-trip,
   excluding the non-round-trippable variants (`Func`, `String8`, `Error`,
   `Object`, `NaN`/`inf` floats, positioned series).

3. `red-eval/tests/programs.rs` — for each `tests/programs/*.red`:
   - Capture stdout, compare to `*.expected`.

4. `red-eval/tests/programs_errors.rs` — for each
   `tests/programs_errors/*.red`: run, assert the rendered `*** Error:` line
   contains the `*.expected` substring.

5. `red-cli/tests/cli.rs` — uses `assert_cmd` to run the binary end-to-end
   (hello world, unbound word, `--version`/`--help`, missing file, REPL via
   stdin, trailing args, `--allow-shell` gating).

6. Inline `#[test]` only for tight unit checks (lexer token kinds, specific
   parser edge cases, `io.rs` url tests marked `#[ignore]`) — kept minimal
   per the "Integration + golden" preference.

A small `tests/common/mod.rs` helper in each crate walks a directory and generates one test per fixture.

## Dependencies (kept minimal)
- `rustyline` (red-cli) — REPL line editing
- `ureq` (red-eval) — http/https fetches for `read url!`
- `proptest` (dev, red-core) — printer/parser round-trip property test
- `tempfile` (dev, red-eval) — scratch dirs for write tests
- `assert_cmd` + `predicates` (dev, red-cli) — CLI tests
- No async, no proc-macros.

## Build/test commands
- `cargo build -p red-cli`
- `cargo test --workspace`
- `cargo run -p red-cli -- examples/hello.red`

## Implementation order (milestones)
1. Scaffold workspace + 3 crate skeletons, empty tests pass
2. `Value` + `Symbol` + `printer` (mold) with unit tests
3. Lexer (token stream + spans), golden round-trip tests added
4. Parser producing `Value::Block`; full round-trip green
5. `Env`/`Context` + minimal `eval` (literals, words, set-words, do) + user-context binding pass
6. Natives: `print`/`prin` first → "hello world" runs end-to-end via CLI
7. Natives: arithmetic, conditionals (`if`/`either`), `loop`/`repeat`/`until`/`while`
8. **Series model**: `Series` cursor, nav/access/mutate natives, `foreach`/`forall`, golden tests
9. **Functions + binding**: `func`/`does`/`return`, `bind`/`use`/`in`/`get`/`set`/`value?`, function-context call frames
10. **`parse` dialect**: matcher subset (`copy`/`set`/`to`/`thru`/`some`/`any`/`opt`/`while`/`|`), string + block input, golden tests
11. REPL mode in CLI
12. Golden program suite for eval; error handling polish
13. v0.2 (M13–M20): refinements, paths, objects, file/shell I/O, strings,
    math/bitwise, conversions, control-flow expansion — see `docs/plans/plan2.md`.

## Decisions confirmed
- Floats: included from the start
- Strings: both `"..."` and `{...}` multiline supported
- REPL: uses `rustyline`
- README: skipped for now
- Series model: full (cursor + mutation); copy-on-write deferred
- Binding: real contexts (user + function + object); closures deferred.
  The binding pass lives in `red-eval/src/binding.rs`, not the parser.
- Functions: in scope (`func`, `does`, `make function!`, `function`, `return`)
- Dialects: no typed framework; a dialect is a native that walks a block.
- `parse`: matcher subset in scope (string + block input);
  `collect`/`keep`/`match`/`case` deferred.
- `compose` is **not** implemented (deferred to v0.3) despite earlier
  mentions; `do`/`reduce` are the only block-walking eval natives.
- Block-integer SetPath (`b/2: 99`) is unreachable from source (lexer gap);
  object-field SetPath works.
- Basic error-as-value (`Value::Error` + `try`/`catch`/`throw`) IS in v0.2;
  the structured error model (`code`/`type`/`args`/`near`/`where`/`by`)
  landed in v0.4 (M42).
