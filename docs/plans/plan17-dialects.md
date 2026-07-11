# Plan 17: Modern Dialects (v0.12) — JSON, HTML, Query, Build

**Baseline:** v0.11 (`plan14-duration.md` M140–M143, `plan15-decimal-type.md`
M150–M154). Current CLI version `0.10.0`; the last milestone is M154.

**Scope:** Four new dialects, each following the `parse.rs` precedent — a native
that walks a block with its own rules. All are additive through the existing
`NativeFn`/`dispatch_block` path; no new `Instr` variants, no new `Value`
variants, no changes to the VM or walker.

**Milestones:** M155–M163 (9 milestones, one per concern).

## Design Summary

| Dialect | Native(s) | New file | Milestones |
|---|---|---|---|
| **JSON codec** | `to-json`, `load-json` | `crates/red-eval/src/json.rs` | M155–M157 |
| **HTML/XML builder** | `html` | `crates/red-eval/src/html.rs` | M158–M159 |
| **Query** | `query` | `crates/red-eval/src/query.rs` | M160–M161 |
| **Build/task** | `build`, `task`, `run-task` | `crates/red-eval/src/build.rs` | M162–M163 |

**Shared patterns (from `parse.rs` research):**

- Each dialect is a `pub fn <name>_native(args: &[Value], refs: &RefineArgs,
  env: &mut Env) -> Result<Value, EvalError>` in its own top-level module
  (not under `natives/`).
- Registration: `register_<dialect>_natives(env: &mut Env)` using
  `crate::natives::reg_refined(env, "name", fn, arity, &[(ref, arity), ...])`,
  wired into `register_natives` in `natives/registry.rs:122`.
- Dispatch: match-based on `crate::series::word_sym(&v)` (not a runtime table).
- Block walking: `extract_series` + `data.borrow()` + `i = series.index` loop
  (the universal pattern from `series.rs:41`).
- Side-effects: `crate::interp::eval(&block, env)` for embedded Red code (the
  `(...)` / `compose` precedent).
- Errors: `EvalError::Native { message, span }` → `enrich_error` wraps for
  `try`/`catch` parity.
- Tests: copy the `BufferWriter` + `run_capture_val` harness from
  `natives/mod.rs:236–273`.
- No binding-pass additions needed: none of these dialects write captures into
  user words at interpretation time (they return values). (The query dialect
  swaps `user_ctx` temporarily for WHERE evaluation but doesn't pre-allocate
  capture slots.)

---

## M155 — JSON Encoder (`to-json`)

**Surface:**

```red
to-json value                    ; → string! (compact)
to-json/pretty value             ; → string! (2-space indented)
to-json/pretty value 4           ; → string! (4-space indented)
```

**Type mapping (value → JSON):**

| Red value | JSON | Notes |
|---|---|---|
| `none` | `null` | |
| `Logic(true/false)` | `true`/`false` | |
| `Integer` | number | `i64::to_string` |
| `Float` | number | JSON conventions (`NaN`/`Inf` → error) |
| `Decimal` | number | Display without `dec` suffix |
| `String` | `"..."` | JSON escapes: `\"` `\\` `\n` `\t` `\r` `\u00XX` |
| `Block` | `[...]` (array) | Walk cursor→tail, recurse each element |
| `Map` | `{"key": val}` | `m.entries.borrow().iter()` in insertion order; all keys → JSON string |
| `Object` | `{"field": val}` | `ctx.words()` (skip `self`), `ctx.get(sym)` per field |
| `Hash` | `{"key": val}` | Same as map but `key_order` side-vec for determinism |
| `Percent` | number | raw fractional value (`50%` → `0.5`) |
| `Money` | string | `"$10.00"` — money has no JSON-native type |
| `Date` | string | ISO 8601: `"2024-06-29T12:30:00+05:30"` |
| `Duration` | number | Seconds as a float (`30s` → `30.0`) |
| `Tuple` | `[255, 0, 0]` | Array of bytes |
| `Pair` | `[100, 200]` | Array of two numbers |
| `Tag` | string | `"<b>"` |
| `Issue` | string | `"ABC"` (without the `#`) |
| `Email` | string | `"foo@bar.com"` |
| `File` | string | `"foo/bar"` (without the `%`) |
| `Url` | string | `"http://..."` |
| `Char` | string | Single-char string |
| `Binary` | string | Base64-encoded |
| Other | **error** | `EvalError::Native` |

**Implementation:**

- New file: `crates/red-eval/src/json.rs`
- `to_json_native(args, refs, env)`: arity 1, `/pretty` refinement (0 or 1 args —
  indent width, default 2).
- A recursive `encode(value: &Value, out: &mut String, indent: usize, pretty:
  bool, indent_width: usize)` function.
- String escaping: `json_escape(s: &str, out: &mut String)` — handles
  `\"`/`\\`/`\n`/`\t`/`\r`/`\u00XX` for control chars < 0x20. For non-ASCII,
  emit the raw UTF-8 char.
- Map key stringification: `map_key_to_json_string(key: &MapKey) -> String`.
- Object field iteration: `ctx.words()` minus `self`, `ctx.get(sym)` per word.
- Registration: `reg_refined(env, "to-json", to_json_native as NativeFn, 1,
  &[("pretty", 1)])` in a new `register_json_natives(env)`.

**Tests (inline `#[cfg(test)]` in `json.rs`):**

- `to_json_scalars` — `to-json 42` → `"42"`, `to-json true` → `"true"`,
  `to-json none` → `"null"`
- `to_json_string_escapes` — `to-json {He said "hi"\n}` → escaped
- `to_json_array` — `to-json [1 2 3]` → `"[1,2,3]"`, `/pretty` indented
- `to_json_map` — `to-json make map! [name "Ada" age 36]`
- `to_json_object` — `to-json make object! [x: 1 y: 2]`
- `to_json_nested` — maps/arrays inside each other
- `to_json_unencodable` — `to-json func [x] [x]` → error

---

## M156 — JSON Decoder (`load-json`)

**Surface:**

```red
load-json "..."              ; → value (map! for objects, block! for arrays)
load-json "..." /only        ; → value (don't wrap top-level in block if it's a scalar)
```

**Type mapping (JSON → value):**

| JSON | Red value | Notes |
|---|---|---|
| `null` | `none` | |
| `true`/`false` | `Logic` | |
| integer | `Integer` | no decimal point or exponent |
| float | `Float` | has `.` or `e`/`E` |
| string | `String` | decode escapes including `\u` |
| array `[...]` | `Block` | recurse each element |
| object `{...}` | `Map` | `make map! [key value ...]` — insertion-ordered |

**Implementation:**

- In the same file: `crates/red-eval/src/json.rs`
- `load_json_native(args, refs, env)`: arity 1 (string! or binary!), `/only`
  refinement (0 args).
- A **hand-rolled recursive descent parser** in Rust (not `parse` — JSON's
  grammar is tiny). Functions: `parse_value`, `parse_string`, `parse_number`,
  `parse_array`, `parse_object`, `parse_literal`.
- Error type: a local `JsonError` enum → converted to `EvalError::Native`.
- `\u` escape handling: read 4 hex digits → `char::from_u32`; handle surrogate
  pairs.
- Number parsing: try `i64` first, fall back to `f64`. Integers that overflow
  `i64` become `Float`.
- Depth guard: `MAX_JSON_DEPTH = 256`.
- Map construction: build `MapDef` directly via `MapDef::new()`,
  `m.set(MapKey::Str(key), value)` per entry.
- Registration: `reg_refined(env, "load-json", load_json_native as NativeFn, 1,
  &[("only", 0)])`.

**Tests:**

- `load_json_scalars` — `load-json "42"` → `42`, etc.
- `load_json_string_escapes` — `load-json "\"hi\\n\""`, `load-json "\"\\u0041\""`
- `load_json_array` — `load-json "[1,2,3]"` → `[1 2 3]`
- `load_json_object` — `load-json {"name":"Ada","age":36}` → `make map! [...]`
- `load_json_nested` — objects/arrays inside each other
- `load_json_round_trip` — `load-json to-json value` == `value` (proptest)
- `load_json_errors` — unterminated string, trailing content, depth exceeded

---

## M157 — JSON Polish, Golden Fixtures, Docs

- `crates/red-eval/tests/programs/json_encode.red` + `.expected`
- `crates/red-eval/tests/programs/json_decode.red` + `.expected`
- `examples/json.red` — demo script
- Update `README.md` and `project-brief.md`

---

## M158 — HTML Builder Dialect (`html`)

**Surface:**

```red
html [
    div class "main" [
        h1 "Welcome"
        p "Hello, " [b "World"] "!"
        ul [li "Item 1" li "Item 2"]
        img src "logo.png" alt "Logo"     ; void element
        br                                 ; void element
    ]
]
; → string! with the rendered HTML
```

**Dialect grammar:**

| Element | Interpretation |
|---|---|
| `Word` (e.g. `div`) | Start of a tag. Collect following `key value` attribute pairs until a `Block` (children) or the next tag. |
| `key value` pairs | HTML attributes: `class "main"` → `class="main"`. |
| `Block` (after attributes) | Child content — recurse. |
| `String` | Text content — append verbatim (HTML-escaped). |
| `Paren` `(expr)` | Evaluate, `form` the result, append (HTML-escaped). |
| Void elements | `area base br col embed hr img input link meta param source track wbr` emit `<tag attrs />`. |

**Implementation:**

- New file: `crates/red-eval/src/html.rs`
- `build_html_native(args, refs, env)`: arity 1 (block!), no refinements.
- `fn render_block(data: &[Value], start: usize, out: &mut String, env: &mut
  Env) -> Result<usize, EvalError>` — recursive.
- `fn render_tag(tag_name: &str, data: &[Value], i: &mut usize, out: &mut
  String, env: &mut Env) -> Result<(), EvalError>`.
- `VOID_ELEMENTS: &[&str]` — const list of HTML5 void element names.
- Registration: `env.natives.insert(Symbol::new("html"), fixed_native(...))`.

**Tests:**

- `html_simple_tag` — `html [p "Hello"]` → `"<p>Hello</p>"`
- `html_attributes` — `html [div class "main" "text"]`
- `html_nested` — nested tags
- `html_void_element` — `html [br]` → `"<br />"`
- `html_text_escape` — `html [p {<script>...}]` → escaped
- `html_paren_eval` — `html [p ("Hello " . name)]`

---

## M159 — HTML/XML Polish, Refinements, Docs

**Refinements for `html`:**

- `/xml` — XML mode: all tags closed, no void elements, XML declaration prefix.
- `/raw` — don't HTML-escape text content.
- `/indent n` — pretty-print with indentation.

**Files:**

- `examples/html.red`
- `crates/red-eval/tests/programs/html.red` + `.expected`
- Update `README.md` and `project-brief.md`

---

## M160 — Query Dialect Core (`query`)

**Surface:**

```red
people: [
    make object! [name: "Alice" age: 30 city: "NYC"]
    make object! [name: "Bob" age: 25 city: "LA"]
    make object! [name: "Carol" age: 41 city: "NYC"]
]

query [
    from people
    select [name age]
    where [age > 20]
    order [age]
]
; → [make object! [name: "Alice" age: 30] make object! [name: "Carol" age: 41]]
```

**Dialect grammar:**

| Keyword | Args | Behavior |
|---|---|---|
| `from` | word-or-block | Resolve the data source. |
| `select` | block-of-words or `*` | Projection. `select *` returns all fields. |
| `where` | block | Filter. Evaluate per-row with fields in scope, `truthy` keeps the row. |
| `order` | block-of-words | Sort by field(s). `order [name]` = asc; `order [name desc]` = desc. |
| `limit` | integer | Take first N rows. |
| `offset` | integer | Skip first N rows. |
| `distinct` | (no args) | Remove duplicate rows. |

**Record formats:**

- `object!` — fields accessed via `ctx.get(sym)`.
- key/value-pair `block!` — `[name "Alice" age 30]` — fields via linear scan.

**WHERE evaluation:**

For each row:

1. Create a fresh child `Context` of `env.user_ctx` (shallow clone, the `use`
   pattern from `natives/words.rs:603–661`).
2. For each field in the row, `child_ctx.set(field_sym, field_value)`.
3. Swap `env.user_ctx` to the child.
4. `dispatch_block(&where_block, env)` → result.
5. Restore `env.user_ctx`.
6. Keep the row if `truthy(&result)`.

**ORDER BY:**

Uses the `sort/compare` pattern from `series.rs:2175–2194`. Build a comparator
that extracts the field value from each record and compares via `num_cmp` /
`values_equal`.

**Implementation:**

- New file: `crates/red-eval/src/query.rs`
- `query_native(args, refs, env)`: arity 1 (block!), no refinements.
- `fn get_field(record: &Value, sym: &Symbol) -> Option<Value>` — handles
  `Object` (via `ctx.get`) and key/value `Block` (via linear scan).
- Registration: `env.natives.insert(Symbol::new("query"), fixed_native(...))`.

**Tests:**

- `query_select_all` — `query [from people]` → all records
- `query_select_fields` — `query [from people select [name]]`
- `query_where` — `query [from people where [age > 30]]`
- `query_where_multiple_fields` — `query [from people where [all [age > 20 city = "NYC"]]]`
- `query_order` — `query [from people order [age]]`
- `query_order_desc` — `query [from people order [age desc]]`
- `query_limit_offset` — `query [from people limit 2 offset 1]`
- `query_distinct` — `query [from people select [city] distinct]`
- `query_chained` — all clauses together
- `query_on_keyvalue_blocks` — records as `[name "Alice" age 30]` blocks

---

## M161 — Query Extensions, Aggregation, Docs

- `count` — shorthand for aggregate count, returns an integer.
- `examples/query.red`
- `crates/red-eval/tests/programs/query.red` + `.expected`
- Update `README.md` and `project-brief.md`

---

## M162 — Build/Task Dialect

**Surface:**

```red
build [
    task clean [print "Cleaning..." delete %target/]
    task compile [print "Compiling..." call "cargo build --release"]
    task test [print "Testing..." call "cargo test"]
    task all [clean compile test]
    default all
]
```

**Dialect grammar:**

| Keyword | Args | Behavior |
|---|---|---|
| `task` | name + body (block) | Register a named task. Stores `TaskDef { name, body, deps }` in `env.tasks`. Does NOT run the body. |
| `default` | name (word) | Mark which task to run by default. |
| `run` | name (word) | Run a task immediately. |

**Task dependencies:**

Inside a task body, bare words that match registered task names are treated as
dependencies — they're run before the rest of the body. Dependencies are run
once per `build` invocation (dedup), with cycle detection.

**Implementation:**

- New file: `crates/red-eval/src/build.rs`
- `Env` additions: `tasks: HashMap<Symbol, Series>` and `default_task:
  Option<Symbol>` (two new fields, default empty).
- `build_native(args, refs, env)`: arity 1 (block!). Walks the block,
  dispatching on `task`/`default`/`run`.
- `run_task_native(args, refs, env)`: arity 1 (word or string). Looks up the
  task, `dispatch_block`s its body. Handles dependency resolution.
- Standalone `task` native: arity 2 (name + body).
- Registration: `register_build_natives(env)` with `build`, `task`,
  `run-task`.

**Tests:**

- `build_registers_tasks`
- `build_default_runs`
- `build_dependencies`
- `build_cycle_detection`
- `build_dedup`
- `build_standalone`

---

## M163 — CLI `--build`, Examples, Docs

- `--build <file.red>` — load the file, evaluate (registering tasks), then run
  the default task. Mutually exclusive with `--disasm` and `--test`.
- `--build <file.red> <task-name>` — run a specific task.
- Exit code: 0 on success, 1 on task failure.

**Files:**

- `examples/build.red`
- `crates/red-eval/tests/programs/build.red` + `.expected`
- `crates/red-cli/tests/cli.rs` — `--build` CLI tests
- Update `README.md` and `project-brief.md`
- `docs/plans/plan17-dialects.md` — this plan document

---

## Open Questions (resolved)

1. **JSON `Percent` encoding:** Emit the raw fractional value (`50%` → `0.5`).
2. **JSON `Binary` encoding:** Base64 string (standard JSON convention).
3. **HTML void element list:** Hardcode HTML5 void elements.
4. **Query `select` output shape:** Objects by default.
5. **Build task dependencies:** Implicit (bare words matching task names).
6. **Codec integration for JSON:** Standalone in v0.12.
7. **Version:** v0.12 "dialects release."
