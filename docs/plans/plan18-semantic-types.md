# Plan 18: Parse-Backed Semantic Types

Implements `docs/plans/future-plan-parse_backed_semantic_types.md`.

Parse-backed semantic types are *schemas over values* — a lightweight, Rebol-native
mechanism for adding domain meaning to compact literal values. A raw datatype
(`tuple!`, `integer!`, `string!`, ...) says what the representation is; a semantic
type (`rgb!`, `port!`, `slug!`, ...) says what the value *means*. Schemas compile
to `parse` rules over the value's *component view*, uniformly across every base
datatype that exposes an extractor.

```rebol
type rgb!: tuple!   [r: byte  g: byte  b: byte]
type port!: integer! [range 1 65535]
type slug!: string! [some slug-char]
type person!: object! [name: string  age: optional [range 0 150]]
```

The values stay ordinary base values at runtime:

```rebol
red: 255.0.0
type? red      ; => tuple!
rgb? red       ; => true
```

and APIs can require specific semantic shapes:

```rebol
paint: func [color [rgb!]] [...]
paint 255.0.0          ; ok
paint 192.168.1.10     ; error: expected rgb!, got tuple! with 4 components
```

## Confirmed decisions

1. **First-class values (Option B).** `rgb!: make semantic-type! [...]` produces a
   `Value::SemanticType(Rc<SemanticTypeDef>>)`. Predicates/constructors are
   generated as natives. A registry on `Env` maps type-name words → definitions.
2. **Extend `TypesetDef`.** A new `semantic: Option<Rc<SemanticTypeDef>>` field
   lets `[rgb!]` annotations reuse the existing M89 walker/VM call-path hooks with
   minimal change: `TypesetDef::accepts(v, env)` checks the base type, then runs
   the semantic parse rule.
3. **Untagged default, optional tagging.** Ordinary literals stay untagged.
   `make rgb! 255.0.0` may optionally tag via a new `Value::SemanticTagged`
   variant. `type?` returns the base type either way; `semantic-type?`
   discriminates.

## Scope

The full plan from `future-plan-parse_backed_semantic_types.md`:

- First-class `semantic-type!` values + registry.
- Pluggable component-extraction protocol with extractors for every supported
  base datatype (`tuple!`, `pair!`, `integer!`/`number!`, `string!`, `binary!`,
  `block!`, `url!`, `object!`, `date!`, `time!`).
- All four schema shapes: positional, scalar, streamed, named.
- Named positional components; `optional` positional components.
- Scalar schemas with `range` and `where`.
- Streamed schemas with `some`/`any`/`optional`, primitive character/token
  constraints, and repetition counts (`N rule`, `lo hi rule`).
- Named schemas with required/optional fields and per-field constraints.
- Dependent constraints (`range 1 days-in-month year month`).
- Generated predicates (`rgb?`) and constructors (`rgb 255 0 0`).
- Function-spec validation (`func [c [rgb!]] [...]`).
- Simple, named error messages.
- Optional tagged semantic values.
- Dialect integration is automatic (a `validate` native + `valid?` is callable
  from any `parse` rule body).

## Out of scope

- Typeset algebra (`union`/`intersect`/`complement`) — plan8 M89 v0.8 territory.
- Static tooling analysis (editor hints, ambiguity warnings) — `future-plan-lsp.md`.
- Documentation generation from type schemas.
- Cross-field validation for object-backed types beyond the dependent-constraint
  mechanism.

## Milestones

M170–M178, additive, each ending green on `cargo test --workspace` and
`cargo test --workspace --features force-walk`.

```
M170 (value + registry) ──► M171 (extractors) ──► M172 (positional + scalar compiler)
                                                    │
                                                    ▼
                                              M173 (streamed + named)
                                                    │
                                                    ▼
                                              M174 (predicates + constructors)
                                                    │
                                       ┌────────────┼────────────┐
                                       ▼            ▼            ▼
                                  M175 (tagged)  M176 (func-spec)  M177 (errors)
                                       └────────────┼────────────┘
                                                    ▼
                                              M178 (optional/counts/dependent + polish)
```

---

## M170 — `semantic-type!` value & registry scaffolding

### Dependencies

- New `Value::SemanticType(Rc<SemanticTypeDef>)` variant in `red-core/src/value.rs`
  (synthetic, no span — like `Typeset`).
- New `SemanticTypeDef` struct in `red-core/src/value.rs`:

  ```rust
  pub struct SemanticTypeDef {
      pub name: Symbol,                  // 'rgb!
      pub base: Symbol,                  // 'tuple!
      pub shape: SemanticShape,          // Positional | Scalar | Streamed | Named
      pub schema: Series,                // the user-facing schema block (for docs/errors)
      pub compiled: RefCell<Option<Rc<Series>>>, // lazy-compiled parse rule (cached)
  }
  pub enum SemanticShape { Positional, Scalar, Streamed, Named }
  ```

- New `Env.semantic_types: HashMap<Symbol, Rc<SemanticTypeDef>>` in
  `red-core/src/env.rs`.
- `type_name_for` → `"semantic-type!"` for the new variant; update `TYPE_WORDS`.

### Tasks

- [x] Add `SemanticTypeDef` + `SemanticShape` in `red-core/src/value.rs`.
- [x] Add `Value::SemanticType(Rc<SemanticTypeDef>)` variant.
- [x] Add `Value::semantic_type(def)` constructor helper.
- [x] Extend `type_name_for` and `TYPE_WORDS` with `semantic-type!`.
- [x] Add `Env.semantic_types` field; initialize empty in `Env::new_with_output`.
- [x] Add `Env::register_semantic_type(&mut self, def: Rc<SemanticTypeDef>)`
      (inserts into `semantic_types` keyed by `def.name`).
- [x] Add `Env::lookup_semantic_type(&self, sym: &Symbol) -> Option<Rc<SemanticTypeDef>>`.
- [x] Printer (`printer.rs`): `mold_semantic_type` — molds as
      `make semantic-type! [name: <nm> base: <base> schema: <mold-of-schema>]`.
- [x] Walker (`interp_walker.rs`) `eval_prefix` self-eval arm:
      `Value::SemanticType(_) => v.clone()`.
- [x] VM compiler (`vm/compiler.rs`) const-pool arm: emit `Value::SemanticType`
      as a constant.
- [x] `make semantic-type! <spec>` constructor in a new
      `crates/red-eval/src/semantic.rs` module. Spec is a block
      `[name: 'rgb!  base: 'tuple!  schema: [r: byte g: byte b: byte]]`. Validates
      `base` is a known type word, derives `shape` from base (via the M171
      shape-of table), stores schema as a `Series`, leaves `compiled` empty
      (lazy). Registers in `env.semantic_types`.
- [x] `semantic-type? value` predicate.
- [x] `to-semantic-type value` converter (accepts an existing `semantic-type!` →
      shallow copy, or a block spec → builds).
- [x] Register the new natives + `make`/`to` dispatch arms in
      `convert.rs::make_native` and `register_convert_natives`.

### Tests (`crates/red-eval/src/semantic.rs` `#[cfg(test)]`)

- [x] `make semantic-type!` round-trips through mold.
- [x] `semantic-type?` true/false.
- [x] Registry: `define-type 'rgb! 'tuple! [r: byte g: byte b: byte]` populates
      `env.semantic_types` (reachable via `valid?` once M172 lands; here just check
      the value molds back).
- [x] `type? make semantic-type! [...]` → `semantic-type!`.

### Docs

- [x] New section in `architecture.md` under "v0.11 additions": "Semantic types
      (M170–M178)".
- [x] Update `README.md` "What's implemented" with a "Semantic types" bullet
      once M172 lands.

---

## M171 — Component-extraction protocol

### Dependencies

- `to-components value` native: dispatches on `type? value` to a registered
  extractor and returns a `Value` suitable as `parse` input (a `block!` for
  positional/scalar/named, a `string!` for streamed-string, a `block!` for
  streamed-block).

### Extractor table (registered on `Env` or as a static function table)

| Base | Returns | Shape |
|---|---|---|
| `tuple!` | `block!` of byte components (as integers) | positional |
| `pair!` | `block!` `[x y]` (integers or floats) | positional |
| `integer!` | `block!` `[n]` (single-element) | scalar |
| `float!`/`decimal!`/`percent!` | `block!` `[n]` | scalar |
| `string!` | the `string!` itself | streamed |
| `binary!` | the `binary!` itself (parse supports it) | streamed |
| `block!`/`paren!` | the block itself | streamed |
| `url!` | `string!` (form of url) | streamed |
| `object!` | `block!` of alternating `word value` pairs (sorted by field order) | named |
| `date!` | `block!` `[year month day]` | positional |
| `time!` (duration) | `block!` `[hours minutes seconds]` | positional |
| fallback | `block!` `[value]` | scalar |

### Tasks

- [x] Add `shape_of(base: &Symbol) -> SemanticShape` in
      `red-core/src/value.rs` (the static dispatch table — pure function of the
      base type word; no `Env` needed since the mapping is fixed).
- [x] Add `to_components(value: &Value) -> Value` in `red-core/src/value.rs`
      (same dispatch). For `object!`, iterate `ObjectDef.ctx` slots in
      declaration order, pushing `Word(name)` then the value.
- [x] Add `to-components value` native in `semantic.rs` that wraps
      `red_core::value::to_components`.
- [x] Register `to-components` (arity 1) in `register_semantic_natives`.

### Tests

- `to-components 255.0.0` → `[255 0 0]`.
- `to-components 100x50` → `[100 50]`.
- `to-components 8080` → `[8080]`.
- `to-components "user-42"` → `"user-42"`.
- `to-components make object! [name: "Ada" age: 36]` → `[name "Ada" age 36]`.
- `to-components 29-Jun-2024` → `[2024 6 29]`.

---

## M172 — Schema compiler: positional & scalar shapes + primitive constraints

### Dependencies

- A library of primitive constraint parse rules. Each primitive is a `Series`
  (block) usable as a parse rule. Defined in `semantic.rs` as `const`-built
  `Series` (built once at module init or lazily).
- A `compile_schema(schema: &Series, base: &Symbol, env: &Env) -> Result<Rc<Series>, EvalError>`
  function in `semantic.rs` that dispatches on `shape_of(base)`.

### Primitive constraint library

Build as Rust-constructed `Series` blocks (not user-script — these are the
*implementation* of the constraint vocabulary). Each is a parse rule block:

- `byte` → `[set n integer! (if not all [n >= 0 n <= 255] [fail]) end]` (emits
  using parse dialect — `set`/`(`/`fail` are already supported per M10/M46).
- `integer` → `[set n integer! end]`
- `non-negative-integer` → `[set n integer! (if n < 0 [fail]) end]`
- `positive-integer` → `[set n integer! (if n <= 0 [fail]) end]`
- `nonzero-integer` → `[set n integer! (if n = 0 [fail]) end]`
- `number` → `[set n [integer! | float!] end]`
- `percent` → `[set n [integer! | float!] (if not all [n >= 0 n <= 100] [fail]) end]`
- `alpha` → `bitset!` rule (charset `#"a"`-`#"z" #"A"`-`#"Z"`)
- `digit` → bitset `0-9`
- `hex-char` → bitset `0-9 a-f A-F`
- `slug-char` → bitset `a-z A-Z 0-9 -`
- `url-char` → bitset (unreserved + `%`)
- `segment` → `[word! | string!]` (a path segment)

Constraints are looked up by name (a `Word` in the schema). The compiler
resolves a `Word` to either a primitive constraint (built-in table) or a
user-defined rule (a `block!` value bound in scope — future, not v1).

### Positional schema compilation

Schema form: `[r: byte  g: byte  b: byte]` — a flat block of `set-word
constraint` pairs.

Compiles to a `Series` block:

```
[set r <byte-rule>  set g <byte-rule>  set b <byte-rule>  end]
```

where `<byte-rule>` is the *inlined* primitive rule body (so
`set r [set n integer! (if not all [n >= 0 n <= 255] [fail])]` — nested). The
`set-word` names (`r`/`g`/`b`) become capture words in the parse rule (parse
already supports `set <word> rule` — M10). They are bound into a scratch context
for error reporting (M176).

### Scalar schema compilation

Schema form: `[range 1 65535]` or `[where [value <> 0]]`.

- `range <lo> <hi>` → `[set n [integer! | float!] (if not all [n >= <lo> n <= <hi>] [fail]) end]`
- `where [predicate]` → `[set n [integer! | float!] (if not (predicate) [fail]) end]`
  — the predicate block is evaluated with `value` bound to `n`.

### Tasks

- [x] Implement `primitive_rule(name: &Symbol) -> Option<Series>` returning the
      inlined rule block for each primitive.
- [x] Implement `compile_positional(schema: &Series, env: &Env) -> Result<Series, EvalError>`:
  - iterate schema pairs of `SetWord` + constraint (a `Word` naming a primitive,
    or a `Block` sub-schema, or a `range`/`where` clause);
  - emit `set <name> <constraint-rule>` for each;
  - append `end`;
  - error on malformed schemas (missing constraint, unknown primitive, wrong
    arity for `range`).
- [x] Implement `compile_scalar(schema: &Series) -> Result<Series, EvalError>`:
  - recognize `range lo hi` and `where [block]` forms;
  - emit the single-element rule.
- [x] Implement `compile_schema(schema, base, env)` dispatching on
      `shape_of(base)`. Cache the result in `SemanticTypeDef.compiled` (a
      `RefCell<Option<Rc<Series>>>`).
- [x] Wire `define-type` to call `compile_schema` eagerly at definition time
      (fail-fast on bad schemas) and store the compiled rule.
- [x] Add `valid? 'type-name value` native:
  - look up `SemanticTypeDef` in `env.semantic_types`;
  - check `type_name(value) == def.base.as_str()` (base type match);
  - run `parse_native(to_components(value), def.compiled)` and return `logic!`.
- [x] Register `valid?` (arity 2).
- [x] Error messages (M176 covers the rich ones; here use simple `Native`
      errors).

### Tests

- Define `rgb!: tuple! [r: byte g: byte b: byte]`; `valid? 'rgb! 255.0.0` → true;
  `valid? 'rgb! 256.0.0` → false; `valid? 'rgb! 1.2` → false (wrong base).
- Define `port!: integer! [range 1 65535]`; `valid? 'port! 443` → true;
  `valid? 'port! 99999` → false; `valid? 'port! "443"` → false.
- Define `ipv4!: tuple! [a: byte b: byte c: byte d: byte]`;
  `valid? 'ipv4! 192.168.1.10` → true; `valid? 'ipv4! 255.0.0` → false
  (3 components).
- Define `size2d!: pair! [width: positive-integer height: positive-integer]`;
  `valid? 'size2d! 100x50` → true; `valid? 'size2d! -5x10` → false.
- Define `percent!: number! [range 0 100]`; covers both integer and float.
- Define `nonzero!: integer! [where [value <> 0]]`; `valid? 'nonzero! 5` →
  true; `valid? 'nonzero! 0` → false.

---

## M173 — Schema compiler: streamed & named shapes

### Streamed schema compilation

Schema form: `[some slug-char]` or `[some alpha "." some alpha]` — the body is a
sequence of parse-dialect expressions (already valid parse rules, since `parse`
is the engine).

Compilation strategy: **the streamed schema body IS the parse rule**, with
minimal transformation:

- Append `end` (semantic types require full consumption).
- Resolve constraint words (`slug-char`, `alpha`, etc.) by inlining their
  bitset/rule definitions at the head of the compiled rule, OR by binding the
  words into a scratch context the parse engine consults. The cleanest is
  **inlining**: replace each constraint `Word` with a `Value::Bitset` or
  `Value::Block` literal in the compiled rule.

For `block!`-streamed, the same approach: the schema is a parse rule body over
block tokens. `segment` inlines to `[word! | string!]`.

### Named schema compilation

Schema form: `[name: string  age: optional [range 0 150]  email: optional email!]`
— `set-word` + constraint, with `optional` marker.

Compiles to a parse rule over the object's field/value pair block:

```
[
  'name set name <string-rule>
  opt ['age set age <range-rule>]
  opt ['email set email <email-rule>]
  end
]
```

where `<string-rule>` is `[string! end]` (single value), `<range-rule>` is the
scalar range rule (without the outer `end` — the named-rule wrapper supplies
it), `<email-rule>` is a recursive `valid?` call compiled to
`(if not valid?('email!, email) [fail])`.

Required fields: if a field's constraint fails (missing or wrong type), the
rule fails. Optional fields: wrapped in `opt`.

### Tasks

- [x] Implement `compile_streamed(schema, base, env)`:
  - clone the schema series;
  - walk and inline each constraint `Word` by substituting its primitive rule
    body (for `alpha`/`digit`/`hex-char`/`slug-char`/`url-char` → a
    `Value::Bitset`; for `segment` → a `Value::Block`);
  - append `Value::word("end")`;
  - return as a `Series`.
- [x] Implement `compile_named(schema, env)`:
  - iterate `SetWord constraint` pairs (with optional `optional` marker before
    the constraint);
  - emit `'name set name <constraint-rule>` for required,
    `opt ['name set name <constraint-rule>]` for optional;
  - a constraint that is itself a semantic type name (`email!`) compiles to a
    recursive `(if not valid?('email!, <captured>) [fail])` — use a `Paren` block
    that calls the `valid?` native;
  - append `end`.
- [x] Extend `compile_schema` dispatch to include `Streamed` and `Named` arms.
- [x] Update `shape_of` if needed (it's already complete from M171).

### Tests

- `slug!: string! [some slug-char]`; `valid? 'slug! "user-42"` → true;
  `valid? 'slug! "Ada Lovelace"` → false; `valid? 'slug! 42` → false (wrong
  base).
- `email!: string! [some alpha-or-digit "@" some slug-char "." some alpha]` —
  define `alpha-or-digit` as a bitset union or a sub-rule. (If the primitive
  vocabulary lacks `alpha-or-digit`, add it.)
- `path!: block! [some segment]`; `valid? 'path! [a b c]` → true;
  `valid? 'path! [1 2 3]` → false (integers aren't segments).
- `csv-row!: block! [field some ["," field]]` — with `field` defined as a string
  segment.
- `person!: object! [name: string age: optional [range 0 150]]`;
  `valid? 'person! make object! [name: "Ada" age: 36]` → true;
  `valid? 'person! make object! [name: 123]` → false;
  `valid? 'person! make object! [name: "Ada" age: 200]` → false.
- `rect!: object! [origin: point2d! size: size2d!]` — nested semantic types
  (requires M174 for `point2d!`/`size2d!` to be defined first; test with both
  defined).

---

## M174 — Generated predicates & constructors

### Tasks

- [x] When `define-type 'rgb! 'tuple! [...]` runs, after registering the
      `SemanticTypeDef`:
  - generate a predicate native `rgb?` (arity 1) that calls
    `valid? 'rgb! value`. Register in `env.natives` as a `Rc<FuncDef>` with
    `native: Some(predicate_fn)`. Since each predicate is a distinct closure,
    use a factory: a single `fn semantic_predicate(args, refs, env)` that reads
    the *call target's* name from the active frame's `func` field and
    dispatches to `valid?`. (This matches how the VM `CallNative` handler
    already accesses `frame.func` for error enrichment.) Alternatively,
    generate a thin `FuncDef` per predicate with a native fn that captures the
    type name via a small side-table — simpler is the frame-func approach.
  - generate a constructor native `rgb` (arity = number of schema components
    for positional; arity 1 for scalar/streamed/named) that validates each
    component against its constraint, builds the base value, and returns it
    **untagged** (tagging is M175). For positional tuple/pair:
    `rgb 255 0 0` → `255.0.0` (a `tuple!`). For scalar: `port 443` → `443` (an
    `integer!`). For streamed: `slug "user-42"` → `"user-42"` (a `string!`,
    already the input). For named: `person "Ada" 36` →
    `make object! [name: "Ada" age: 36]`.
- [x] The constructor validates *before* building — e.g. `rgb 256 0 0` raises
      `EvalError::Native` with a component-named error (M176).
- [x] Register generated natives in `env.natives` (overwriting any existing
      user word — documented: `define-type` shadows existing natives with the
      same name as the semantic type's bare word).
- [x] Make `define-type` itself a native (arity 3: name, base, schema) — it's
      the public surface for type definition.

### Tests

- After `define-type 'rgb! 'tuple! [r: byte g: byte b: byte]`:
  - `rgb? 255.0.0` → true; `rgb? 192.168.1.10` → false.
  - `rgb 255 0 0` → `255.0.0`; `type? rgb 255 0 0` → `tuple!`.
  - `rgb 256 0 0` → error "component r must be byte (0-255), got 256".
- After `define-type 'port! 'integer! [range 1 65535]`:
  - `port? 443` → true; `port? 99999` → false.
  - `port 443` → `443`; `port 99999` → error.
- After `define-type 'slug! 'string! [some slug-char]`:
  - `slug? "user-42"` → true.
  - `slug "user-42"` → `"user-42"`.
- After `define-type 'person! 'object! [name: string age: optional [range 0 150]]`:
  - `person "Ada" 36` → `make object! [name: "Ada" age: 36]`.
  - `person? person "Ada" 36` → true.

---

## M175 — Tagged semantic values

### Design

Add `Value::SemanticTagged { tag: Symbol, value: Box<Value>, span: Span }` — a
value carrying its semantic type tag. `type?` returns the **base** type
(unwrap the inner value); `semantic-type?` returns the tag word; `mold`
renders the untagged form by default (so serialization stays simple per the
plan), with an optional `/tagged` refinement on `mold` that emits
`make <tag>! <inner-mold>`.

Equality: two `SemanticTagged` values are `equal?` iff tags match AND inner
values are `equal?`. A `SemanticTagged` and a plain value with the same inner
value are `equal?` (value equality, not semantic equality — matches the plan's
"value equality: true, semantic equality: false" resolution for cross-tag
comparisons, but same-tag uses strict equality). Document this.

### Tasks

- [ ] Add `Value::SemanticTagged { tag: Symbol, value: Box<Value>, span: Span }`
      variant in `value.rs`.
- [ ] `type_name_for` → unwrap inner, return base type name.
- [ ] Update `TYPE_WORDS` (no new entry — tagged values share the base type
      name; `semantic-type?` is the discriminator).
- [ ] Walker `eval_prefix`: `Value::SemanticTagged { .. }` self-evaluates
      (returns clone).
- [ ] VM const-pool: emit as a constant.
- [ ] Every `match v` arm in the codebase that handles base types
      (`Value::Integer`, `Value::Tuple`, etc.) must also handle
      `Value::SemanticTagged` by **unwrapping** — OR add a single
      `unwrap_semantic(v) -> &Value` helper and audit the hot paths. Pragmatic
      approach: add the variant, then update `type_name_for`, `mold`, `form`,
      `compare.rs` equality, arithmetic, series pick/poke, path resolution
      (`pair/x`, `tuple/r`). Most of these already go through `type_name_for`
      or a central dispatch; the audit is mechanical.
- [ ] `make <semantic-type-name>! <value>` in `make_native`: when the type
      operand is not a builtin but IS in `env.semantic_types`, validate via
      `valid?` and return
      `Value::SemanticTagged { tag: name, value: Box::new(spec.clone()), span }`.
      On validation failure, raise `EvalError::Native` with a rich error (M176).
- [ ] `semantic-type? value`:
  - if `Value::SemanticType(_)`: return the type name as a `lit-word!`.
  - if `Value::SemanticTagged { tag, .. }`: return the tag as a `lit-word!`.
  - else: return `none`.
- [ ] `mold`/`form`: by default unwrap (render the inner value). Add
      `mold/tagged` refinement (0-arity on `mold` — already has refinements? if
      not, register `mold` with `/tagged`) that renders `make <tag>! <inner-mold>`
      for `SemanticTagged` values.
- [ ] Constructor `rgb 255 0 0` stays **untagged** (per plan default).
      `make rgb! 255.0.0` is **tagged**. Document the distinction.
- [ ] `copy` of a `SemanticTagged` preserves the tag.

### Tests

- `red: make rgb! 255.0.0` → `type? red` = `tuple!`; `semantic-type? red` =
  `'rgb!`.
- `rgb? red` → true (predicates accept tagged OR untagged — they call `valid?`
  which unwraps).
- `mold red` → `"255.0.0"` (untagged default).
- `mold/tagged red` → `"make rgb! 255.0.0"`.
- `make rgb! 256.0.0` → error.
- `equal? make rgb! 1.2.3 make semver! 1.2.3` → true (value equality, different
  tags).
- `same? make rgb! 1.2.3 make rgb! 1.2.3` → false (distinct allocations).
- Arithmetic on tagged tuple: `make rgb! 255.0.0 + 1.0.0` → `255.0.0`
  (untagged result — arithmetic unwraps).

---

## M176 — Func-spec integration with `TypesetDef`

### Dependencies

- Extend `TypesetDef` in `red-core/src/value.rs`:

  ```rust
  pub struct TypesetDef {
      pub types: RefCell<HashSet<Symbol>>,
      pub semantic: RefCell<Option<Rc<SemanticTypeDef>>>,  // new
  }
  ```

- `TypesetDef::accepts(v, env)` extended: after the existing base-type check
  passes, if `semantic` is `Some(def)`, also run
  `parse(def.compiled, to_components(v))` and return that result. If the
  base-type check fails, return false (don't run the parse rule).

### Func spec parsing

`extract_spec` in `natives/func.rs` calls `parse_typeset_block(v)` for a
`[type! ...]` annotation block. Extend `parse_typeset_block` to:

1. For each `Word`/`LitWord` in the block: if it's a known builtin type word →
   add to `types` set (existing behavior).
2. If it's NOT a builtin but IS in `env.semantic_types` → set
   `semantic = Some(def)` AND add `def.base` to `types` (so the base-type check
   in `accepts` is the first gate).
3. If it's neither → existing error ("unknown type word").

`parse_typeset_block` currently takes only `&Value`; change signature to
`parse_typeset_block(block: &Value, env: &Env) -> Result<Rc<TypesetDef>, EvalError>`
so it can consult `env.semantic_types`. Update callers in `func.rs`
(`extract_spec`) and `typeset.rs` (`make_typeset`). `extract_spec` already has
access to `env` (it's called from `func_native`/`function_native`/
`closure_native` which receive `&mut Env` — but `extract_spec` currently
doesn't take `env`; thread it through).

A typeset with a semantic ref may contain ONLY that one semantic type (mixing
`[rgb! integer!]` is meaningless — a value can't be both an rgb tuple and a
bare integer). If a block has a semantic type word plus others, error: "semantic
type <name> cannot be mixed with other type words in a typeset".

### Tasks

- [x] Add `semantic: RefCell<Option<Rc<SemanticTypeDef>>>` to `TypesetDef`;
      update `new`/`from_words`/`Default` to initialize it `None`.
- [x] Extend `TypesetDef::accepts`: after base check, if `semantic` is `Some`,
      run `parse(to_components(v), def.compiled)`. The parse call needs
      `&mut Env` — so `accepts` must take `&Env` (read-only access to
      `semantic_types` for compiled-rule lookup is fine; the parse run itself
      needs `&mut Env` for side-effect evaluation). Change signature to
      `accepts(&self, v: &Value, env: &mut Env) -> bool`.
- [x] Update all callers of `TypesetDef::accepts`: walker `check_param_types`
      (has `&mut Env`), VM `prepare_call` inline check (has `&mut Env`),
      `copy/types` in `series.rs` (M136 — has `&mut Env`). Audit via
      `grep accepts`.
- [x] Thread `env: &Env` (or `&mut Env`) through `extract_spec` and
      `parse_typeset_block`.
- [x] Update `make typeset!` to accept a semantic-type word and populate the
      `semantic` field (error on mixed blocks).
- [x] The M89 error message path (`"type error: arg N expected [ts], got <found>"`)
      should, for a semantic-typeset failure, render
      `"type error: arg N expected rgb!, got <found>: <reason>"` using M177's
      rich errors.

### Tests

- `f: func [c [rgb!]] [c]  f 255.0.0` → `255.0.0`.
- `f 192.168.1.10` → error "expected rgb!, got tuple! with 4 components".
- `f "red"` → error "expected rgb! (base tuple!), got string!".
- `f: func [p [port!]] [p]  f 443` → `443`; `f 99999` → error;
  `f "443"` → error.
- `f: func [s [slug!]] [s]  f "user-42"` → `"user-42"`; `f "a b"` → error.
- `f: func [a [ipv4!] p [port!]] [a]  f 192.168.1.10 443` → works.
- Back-compat: existing `func [x [integer!]]` fixtures stay green (no semantic
  ref → unchanged path).

---

## M177 — Rich error reporting

### Tasks

- [x] Add a `SemanticError` helper struct (in `semantic.rs`): carries
      `type_name: Symbol`, `component: Option<Symbol>` (the field name, for
      positional/named), `index: Option<usize>` (for streamed),
      `expected: String`, `got: String`, `span: Span`.
- [x] The schema compiler emits rules that, on failure, capture *which*
      component/constraint failed. Approach: instead of bare `fail`, use
      `(component-error 'r 'byte n)` — a paren block that calls a native
      `component-error` which raises `EvalError::Native` with a formatted
      message. The `set r <rule>` captures the value into `r`; if the rule's
      inner check fails, the error native names `r`.
- [x] For positional: `"Invalid rgb!: component r must be byte (0-255), got 256"`
      or `"Invalid ipv4!: expected 4 components, got 3"`.
- [x] For scalar: `"Invalid port!: expected integer in range 1..65535, got 99999"`
      or `"Invalid port!: expected integer!, got string!"`.
- [x] For streamed: `"Invalid slug!: disallowed character ' ' at index 3"` —
      track the cursor index on failure (parse exposes position via the input
      cursor; on rule failure, the native reads the current cursor).
- [x] For named: `"Invalid person!: missing required field name"` or
      `"Invalid person!: field age must be in range 0..150, got 200"`.
- [x] `valid?` returns `logic!` (no error) for public use; the rich errors
      surface from **constructors** (`rgb 256 0 0`) and **func-spec checks**
      (which raise on failure). Add a `validate 'type value` native (distinct
      from `valid?`) that raises a rich error on failure — used by constructors
      and the func-spec path.

### Tests

- Each error shape above, asserted via `err_src(...).contains(...)`.

---

## M178 — Optional positional, repetition counts, dependent constraints, polish & release

### Optional positional components

Schema: `[major: integer minor: integer patch: optional integer]`. Compiler
emits `opt [set patch <int-rule>]` before `end`. `version: 1.4.2` validates;
`1.4` also validates (patch absent).

### Repetition counts

Schema: `[3 segment]` (exactly 3) or `[2 to 5 alpha]` (between 2 and 5).
Compiler emits parse's `3 rule` / `2 5 rule` forms (parse already supports
count + rule per M10).

### Dependent constraints

Schema: `[year: integer  month: range 1 12  day: range 1 (days-in-month year month)]`
— the `range`'s hi operand may be a `Paren` block evaluated at check time with
the already-captured components in scope. Compiler emits
`(if not all [day >= 1 day <= (days-in-month year month)] [fail])`. The captures
(`year`, `month`) are bound in the parse scratch context.

### Polish

- [ ] `docs/` — write `docs/semantic-types.md` user guide (mirrors the plan's
      examples).
- [x] `examples/semantic-types.red` — demo script.
- [ ] Golden fixtures: `crates/red-eval/tests/fixtures/semantic-*.red` +
      `.expected` (10+ covering each shape, errors, tagged values).
- [x] Update `architecture.md` with the full M170–M178 section.
- [x] Update `README.md` "What's implemented" + add a "Semantic types"
      subsection.
- [ ] `project-brief.md` — add semantic types to the value model section.
- [x] `cargo test --workspace` green; `cargo test --workspace --features force-walk`
      green (walker parity).
- [x] `cargo bench` sanity (no regression on `fib 30` / `sum_loop` — the
      `accepts` path adds one `Option::is_some` check per call; measure).
      Result: `fib 30` +2.5% (530ms, within noise of code-layout change);
      `sum_loop` unchanged; most fixtures improved.

### Tasks

- [x] Extend `compile_positional` for `optional` marker.
- [ ] Extend `compile_streamed`/`compile_positional` for count forms
      (`N constraint`, `lo hi constraint`).
- [ ] Extend `range`/`where` to accept `Paren` operands evaluated with
      captured components in scope (needs the parse scratch context to be the
      function-local context of a synthetic func — wire via `env`).
- [ ] Implement `days-in-month year month` helper native (for the `iso-date!`
      example).
- [x] `make <semantic-type>! <value>` construction (standard Rebol pattern).
- [x] `validate 'type value` native with rich error messages.

---

## Risk areas

1. **`Value::SemanticTagged` audit:** adding a new `Value` variant touches every
   exhaustive `match` in the codebase. Mitigation: add the variant, run
   `cargo build`, fix each compiler error by unwrapping in the base-type arm.
   Mechanical but broad (~30–50 match sites).
2. **`TypesetDef::accepts` signature change** (now needs `&mut Env` for the parse
   run): threads through the hot function-call path. Mitigation: the
   `semantic.is_none()` fast path (the vast majority of typesets) skips the env
   borrow entirely — one `Option::is_none` check per call, matching the
   existing M89 `param_types.is_empty()` fast path.
3. **Parse rule construction in Rust:** building `Series` blocks programmatically
   (for primitive constraints) is verbose. Mitigation: a small `rule!` macro or
   builder helpers in `semantic.rs` (`rule_set(&[w])`, `rule_if(&[...])`, etc.).
4. **Named schema field ordering:** `to-components` on an object must emit fields
   in a stable order. `ObjectDef.ctx` slots are insertion-ordered (per M18) — use
   that.

## Sequencing recap

M170–M174 are strictly sequential. M175/M176/M177 can be done in parallel after
M174 (they're orthogonal concerns). M178 is the release milestone.
