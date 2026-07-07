# Plan 16: Red-by-Example Parity Gaps (Non-GUI)

**Source:** Gap analysis against https://www.red-by-example.org/ "Red words in
alphanumerical order" master list + category index.
**Scope:** Core language natives/actions/datatypes/constants/operators only.
GUI (VID/View), Draw Dialect, Parse dialect, Events, Colors categories are
**out of scope** per project standing policy. Additional scope decisions
(recorded below) were made interactively during planning.

## Scope decisions (interactive)

- **Abstract/supertype datatype words:** INCLUDED as gaps (we have most
  predicates like `any-block?` but not the datatype words themselves).
- **Reactivity** (`react`/`react?`/`clear-reactions`/`stop-reactor`): left to
  the existing `future-plan-reactivity.md`; not tracked here.
- **FFI-adjacent types** (`routine!`/`routine`, `external!`, `handle!`,
  `ref!`): OUT OF SCOPE — deferred alongside the `routine!` FFI binding layer.
- **Aliases** (`arccosine`/`arcsine`/`arctangent`/`arctangent2`, `sine`/
  `tangent`, `cd`/`pwd`/`ll`/`ls`/`list-dir`, `yes`/`no`/`on`/`off`, `q`,
  `create-dir`, `get-current-dir`, `browse`): NOT added — low value for a POC.
- **`event!`/`event?`:** OUT OF SCOPE (GUI-adjacent value type).
- **Clipboard I/O** (`read-clipboard`/`write-clipboard`): out of scope
  (GUI/runtime-adjacent).

## Methodology

Every word in the Red-by-example "Red words" master list (441 entries after
excluding GUI/Draw/Parse-dialect/Events/Colors categories) was cross-referenced
against:

- `crates/red-eval/src/natives/registry.rs` (native registration hub)
- per-concern modules under `crates/red-eval/src/{natives/*,series,strings,
  math,parse,object,map,hash,vector,image,bitset,typeset,codec,path,io,module,
  net,reflection,convert}.rs`
- the `Value` enum in `crates/red-core/src/value.rs`
- the stdlib at `crates/red-eval/stdlib/stdlib.red`

Words we already implement are not listed. What remains is organized below by
category, with notes on each gap.

---

## 1. Datatype Words — Abstract/Supertype Types Missing as First-Class Words

We have most of the *predicates* (`any-block?`, `series?`, etc.) but Red also
exposes these as **datatype words** usable in `typeset!` blocks, `func
[x [series!]]` spec-matching, and returned by `types-of`. These words are
absent from the type system.

| Missing word | Notes |
|---|---|
| `series!` | supertype of block/paren/string/binary/hash/vector/map/port |
| `number!` | supertype of integer/float/decimal/percent/money |
| `any-block!` | block/paren/path/get-path/lit-path/set-path |
| `any-string!` | string/binary |
| `any-path!` | path/get-path/lit-path/set-path |
| `any-word!` | word/set-word/get-word/lit-word/refinement |
| `any-list!` | block/paren |
| `any-object!` | object/module/error |
| `any-function!` | function/native/op/closure |
| `any-type!` | universal |
| `scalar!` | all scalars (numbers/percent/money/char/pair/tuple/date…) |
| `immediate!` | scalars + none/unset/logic |
| `internal!` | internal types |
| `all-word!` | all word kinds |
| `datatype!` | the type of type words themselves |
| `time!` | **concrete** type — we fold time into `date!`; Red has a standalone `time!`. Real gap. |

**Action:** Add these as first-class type words recognized by `typeset!`,
spec-matching, and `types-of`. Standalone `time!` is the largest item
(requires a new `Value::Time` variant or splitting the time component out of
`Date`).

---

## 2. Missing Natives & Actions

### 2a. Series / String (high traffic)

| Word | Category | Notes |
|---|---|---|
| `alter` | Series | append-if-absent |
| `extract` | Series | pick every nth into block |
| `fifth` / `fourth` | Series | positional accessors (we have first/second/third/last) |
| `move` | Series | move range between series |
| `new-line` | Series | set new-line head flag (we have `new-line?` only) |
| `offset?` | Series | distance between two series positions |
| `repend` | Series | reduce + append |
| `pad` | Formatting | pad string to width |
| `a-an` | String | "a"/"an" grammar helper |
| `ellipsize-at` | String | truncate with ellipsis |

### 2b. Conversion / Casting

| Word | Notes |
|---|---|
| `as` | non-coercing reinterpret (vs `to`) |
| `as-color` / `as-rgba` / `as-pair` / `as-ipv4` | fast tuple/pair constructors |
| `dehex` | decode %XX in strings |
| `hex-to-rgb` | color conversion |
| `modify` | in-place facet modification |
| `dirize` | ensure trailing slash on file!/string |
| `to-hex` | integer→hex string |
| `to-get-path` / `to-lit-path` / `to-set-path` | path-kind converters (only `to-path` exists) |
| `to-paren` | block→paren |
| `to-refinement` | word→refinement |
| `to-none` / `to-unset` | typed no-value conversions |
| `to-local-file` / `to-red-file` | OS-path ↔ Red file! |
| `to-local-date` / `to-UTC-date` | date zone conversions (we have non-standard `to-utc`) |
| `to-time` | build time! value (blocked on §1 `time!`) |

### 2c. Path / Directory utilities

| Word | Notes |
|---|---|
| `clean-path` | normalize `.`/`..` |
| `normalize-dir` | canonicalize dir form |
| `split-path` | split file! → dir + basename |
| `dir` | current-dir as file! |
| `set-current-dir` | explicit setter (we have `change-dir`) |
| `query` | file metadata query |

### 2d. Operators (infix)

| Word | Notes |
|---|---|
| `<<` / `>>` / `>>>` | infix shift (we have prefix `shift-left`/`shift-right`/`shift-logical` only) |
| `and~` / `or~` / `xor~` | destructive bitwise-assignment variants |
| `%` | infix modulo (distinct from `//`) |
| `==` / `=?` | infix strict-equal / same? (predicates exist, symbols don't) |

### 2e. Math

| Word | Notes |
|---|---|
| `absolute` | canonical name (we have `abs`) |
| `average` / `sum` | block reduction helpers |
| `mod` / `modulo` / `remainder` | distinct modulo flavors (we have `//` only) |
| `math` | precedence-aware math expression evaluator |
| `within?` | point-in-bounds test (pair/tuple) |
| `NaN?` | float NaN test |

### 2f. Evaluation / Control

| Word | Notes |
|---|---|
| `also` | pass-through pipe (returns first arg) |
| `do-safe` | sandboxed do |
| `eval-set-path` | set-path evaluation hook |
| `halt` | stop interpreter |
| `quit-return` | quit with exit code |
| `quote` | return literal value (word/value) |
| `construct` | object constructor (alternate `make object!`) |

### 2g. Help / Documentation / Reflection (whole subsystem missing)

| Word | Notes |
|---|---|
| `?` | help word |
| `??` | debug-print word (we have `??` in parse only) |
| `help` / `fetch-help` / `help-string` | help system |
| `source` | print native/func source |
| `what` | list defined words |
| `about` | version/about info |

### 2h. System / Environment

| Word | Notes |
|---|---|
| `list-env` | list environment vars (we have `get-env`/`set-env`/`env`) |
| `os-info` | OS metadata object |
| `stats` | interpreter statistics |
| `recycle` | GC noop (we have no GC) |
| `rebol` | compat shim — likely skip |
| `extract-boot-args` / `flip-exe-flag` | launcher internals — low priority |
| `write-stdout` | low-level stdout write (we have `prin`/`print`) |
| `last-lf?` | trailing-newline state — low priority |

### 2i. Macros

| Word | Notes |
|---|---|
| `expand` / `expand-directives` | Red macro/preprocessor system — sizable feature |

### 2j. Input (console)

| Word | Notes |
|---|---|
| `ask` | prompt + read line |
| `input` | read line from stdin |
| `red-complete-input` | REPL completion hook — low priority |

### 2k. Network cache (`*-thru` family)

| Word | Notes |
|---|---|
| `read-thru` / `load-thru` / `do-thru` | fetch+cache remote resource |
| `exists-thru?` / `path-thru` | cache queries |

*Scope note: included since they're tied to the existing networking layer
(not GUI). Flag if you'd rather drop them.*

### 2l. Parse

| Word | Notes |
|---|---|
| `parse-trace` | traced parse execution for debugging |

---

## 3. Constants Missing

Red seeds these into the global context; we only seed `none`, `unset`,
`true`, `false`, `newline`, `pi`, `e`, `system`.

| Missing | Value |
|---|---|
| `tab` | `#"^-"` / `"\t"` |
| `sp` / `space` | `" "` |
| `cr` | `"^M"` / `"\r"` |
| `lf` | `"^/"` / `"\n"` |
| `slash` | `"/"` |
| `dot` | `"."` |
| `escape` | `#"^["` |
| `comma` | `#","` |
| `dbl-quote` | `"^"` |
| `null` | char/none — Red uses for null pointer in FFI; low priority given FFI excluded |

---

## 4. Naming Mismatches (we use non-canonical names)

These aren't "missing" per se, but Red's canonical name differs from ours.
Worth renaming or adding the canonical as an alias even though aliases are out
of scope — these are cases where we deviated from Red's name, not extra
aliases.

| Ours | Red canonical |
|---|---|
| `to-utc` | `to-UTC-date` |
| `abs` | `absolute` (Red has `absolute`; `abs` may also exist) |

*Recommend: add the canonical names; keep ours as aliases for back-compat.*

---

## 5. Confirmed Out of Scope (per decisions + standing README)

- **GUI:** VID/View, Draw, Events, Colors, `event!`/`event?`,
  `request-dir`/`request-file`/`foreach-face`, clipboard I/O.
- **Reactivity:** `react`/`react?`/`clear-reactions`/`stop-reactor` → tracked
  in `future-plan-reactivity.md`.
- **FFI-adjacent:** `routine!`/`routine`, `external!`, `handle!`, `ref!` →
  deferred with the routine FFI layer.
- **Aliases:** `arccosine`/`arcsine`/`arctangent`/`arctangent2`, `sine`/
  `tangent`, `cd`/`pwd`/`ll`/`ls`/`list-dir`, `yes`/`no`/`on`/`off`, `q`,
  `create-dir`, `get-current-dir`, `browse`.

---

## 6. Proposed Execution Order (by impact × low effort)

1. **Constants** (§3) — trivial, high visibility.
2. **Series/String gaps** (§2a) — `alter`/`extract`/`fifth`/`fourth`/`move`/
   `new-line`/`offset?`/`repend`/`pad`.
3. **Conversion gaps** (§2b) — `to-hex`/`to-*path`/`to-paren`/`dehex`/
   `dirize`/`modify` + canonical `to-UTC-date`.
4. **Path/dir utilities** (§2c) — `clean-path`/`normalize-dir`/`split-path`/
   `query`.
5. **Math gaps** (§2e) — `average`/`sum`/`mod`/`modulo`/`remainder`/
   `absolute`/`within?`/`NaN?`.
6. **Infix operators** (§2d) — `<<`/`>>`/`>>>`/`and~`/`or~`/`xor~`/`%`/
   `==`/`=?`.
7. **Abstract datatype words** (§1) — typeset/spec integration.
8. **Eval/control** (§2f) — `also`/`halt`/`quit-return`/`quote`/`do-safe`/
   `eval-set-path`/`construct`.
9. **Console input** (§2j) — `ask`/`input`.
10. **Help subsystem** (§2g) — `?`/`help`/`source`/`what`/`about` (larger
    design effort).
11. **System info** (§2h) — `list-env`/`os-info`/`stats`/`write-stdout`.
12. **Network cache** (§2k) — `*-thru` family.
13. **`time!` datatype** (§1) — split out of `date!`; largest single item,
    do last.
14. **Macros** (§2i) — `expand`/`expand-directives`; deferred unless needed.
