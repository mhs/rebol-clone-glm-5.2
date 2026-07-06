# Plan 10: Built-in Unit Testing (v0.5.x)

Execution checklist extending the v0.5.0 baseline in `plan6-closures-modules.md`.
This lands a **first-class `test` dialect** — a `red --test` CLI mode backed by
Rust-side `test`/`assert-*` natives, with `suite` grouping, `before-test`/
`after-test` hooks, and TAP-14 reporting. Test natives are always-on (not gated
by stdlib auto-import); the pure-Red stdlib facade (Option A) is intentionally
**not** added — the natives are the canonical surface, mirroring how `if`/`try`
are unconditional language built-ins.

Per `project-brief.md`, every new construct is additive through the existing VM
`Call`/`CallUser`/native-call path; the v0.3.3 VM stays the default evaluator.
The parity harness (`tests/parity.rs`) and `cargo test --workspace --features
force-walk` remain the regression gates.

Deferred to v0.6+ (acknowledged, not built here): parallel test execution
(`Env` is `!Send`), `--test` filtering (`--filter name`, `--only-failing`),
JUnit XML output, watch mode (`--test --watch`), snapshot/property-test
assertions, in-REPL test running, fuzzing the test natives themselves beyond
the existing `fuzz/` harness coverage.

## Design summary

Three themes, in priority order:

1. **Collection-then-run semantics.** `test` and `suite` *register* into an
   `Env`-level registry during evaluation of the script body; they do **not**
   execute their bodies inline. After the script's top-level evaluation
   completes (in `--test` mode), the runner drains the registry and executes
   each test body once in an isolated child `Context`. This avoids
   double-execution of side-effecting top-level code and lets `before-test`/
   `after-test` hooks compose across nested suites.

2. **Structured failures via existing `error!` values.** `assert-*` and `fail`
   raise `EvalError::Raised(Rc<ErrorValue>)` with `type: 'test` and a
   one-line `message:` (for `assert-equal`, the message includes `mold` of
   both operands). `run-tests` wraps each test body in `try` (existing
   `try_native`, `natives/control.rs`); a caught `error!` is a test failure,
   an uncaught error (lex/parse/`exit`) is a *crash* — distinguishable in the
   TAP output via a `# crash` directive.

3. **Isolation by child `Context`.** Each test body runs in a fresh child of
   `user_ctx` (via `bind_pass` into a child `Context`, mirroring `use`'s
   pattern at `natives/words.rs:603–661`). SetWords inside a test don't leak to
   the next test. `before-test` runs in the same child context as the test;
   `after-test` runs in the same child context too, even on failure (the
   runner wraps the test+after in a `finally`-style helper — Red has no
   `finally`, so this is a thin Rust wrapper inside `run_tests_native`).
   Nested suites inherit their parent's hooks; child hooks run *after* parent
   hooks (so a parent `before-test` sees a clean state, a child `before-test`
   sees the parent's setup).

Non-goals: a separate `test!` value type (tests are registered in `Env`, not
carried as values), a new `Instr` for test dispatch (tests run via the existing
`dispatch_block` / `try` path), reactivity integration, async tests,
parameterized tests (`test "name" [args] [body]`), or `--test`-with-`--disasm`
(a `--disasm --test` combo errors out — tests don't have a single bytecode
entry point).

## Ground-truth references (from research)

- `Env` struct (`crates/red-core/src/env.rs:143`), `new_with_output`
  (`env.rs:285`). Fields like `modules` (`env.rs:236`), `stdlib`
  (`env.rs:261`) show the precedent for `Env`-level registries.
- `register_natives` is the single registration site
  (`crates/red-eval/src/natives/registry.rs:115`); helpers `fixed_native`
  (`registry.rs:40`), `infix_native` (`registry.rs:53`), `variadic_native`
  (`registry.rs:68`), `reg_refined` (`registry.rs:83`) cover all native
  shapes needed by the test words.
- `RunOptions` (`crates/red-eval/src/interp_runner.rs:69`) already carries
  `allow_shell`, `walk`, `trace`, `no_stdlib`, `module_paths`, `args`. Adding
  `test_mode: bool` is one line; the dispatch tail at `interp_runner.rs:213`
  is the place to auto-invoke `run_tests`.
- `EvalError::Quit(code)` exit-code path (`interp_runner.rs:215`) — the runner
  returns `Ok((Value::None, code))`; the CLI (`crates/red-cli/src/main.rs:162`)
  propagates this to the process exit status. `--test` mode reuses this for
  the pass/fail exit code (0 on all-pass, 1 on any failure).
- `EvalError::Raised(Rc<ErrorValue>)` (`natives/mod.rs:62` `enrich_error`,
  `value.rs` `ErrorValue::new_structed`) is the structured-error path
  `assert-*`/`fail` will raise. `ErrorValue::new_structed(message, code,
  type, args, near, where, by)` is the constructor (see
  `natives/mod.rs:95–104` for the call shape).
- `try_native` / `catch_native` (`natives/control.rs`) catch `Raised` as a
  `Value::Error`; `run_tests_native` will use the same mechanism per test.
- `use_native` (`natives/words.rs:603–661`) shows the child-`Context` swap
  pattern (`env.user_ctx` replaced, body evaluated, restored). The test
  runner uses the same pattern, but with a fresh child per test (not a clone).
- CLI flag parsing is hand-rolled (`crates/red-cli/src/main.rs:72–105`);
  `--test` slots in as a boolean flag alongside `--walk`/`--trace`/`--no-stdlib`.
  The `--disasm` / `--disasm-func` precedence block (`main.rs:108–114`) is the
  template for a `--test` precedence check (`--test` + `--disasm` errors).
- `ErrorValue` fields: `code`/`type`/`message`/`args`/`near`/`where`/`by`
  (see `natives/mod.rs:95–104` call site, `value.rs` definition). The test
  dialect uses `type: 'test`, `message: <summary>`, `args: [found expected]`
  (for `assert-equal`), `where: 'assert-equal` (the asserting word).
- `mold_to_string` (`crates/red-core/src/printer.rs`) is the canonical value
  renderer for diagnostic messages.
- `BufferWriter` test pattern (`crates/red-eval/tests/common/mod.rs:78`) is
  how inline `#[test]`s capture stdout — the new tests in `natives/test.rs`
  will reuse the same `Rc<RefCell<Vec<u8>>>` sink pattern (see
  `natives/mod.rs:206–216` for an inline-test adaptation).
- `tests/programs.rs` golden harness (`crates/red-eval/tests/programs.rs:13`)
  pairs `.red` with `.expected` — *not* used for `--test` mode (TAP output is
  generated, not golden-matched), but the pattern is referenced for the
  optional future TAP-golden suite.

## Language surface

New always-on native words (registered in `register_natives`, not via stdlib):

```red
test "name" [body]                 ; arity 2: String! + Block! — register a test
suite "name" [body]                ; arity 2: String! + Block! — open a group
before-test [body]                 ; arity 1: Block! — hook for current suite
after-test  [body]                 ; arity 1: Block! — hook for current suite
assert [cond]                      ; arity 1: Block! — fail if cond falsy
assert-equal [a b]                 ; arity 1: Block! of 2 vals — fail if `a <> b`
assert-not-equal [a b]             ; arity 1: Block! of 2 vals — fail if `a = b`
assert-error [body]                ; arity 1: Block! — fail if body does NOT raise
assert-no-error [body]             ; arity 1: Block! — fail if body raises
fail "reason"                      ; arity 1: String! — unconditional failure
run-tests                          ; arity 0 — drain registry, print TAP, exit-code side effect
```

Notes:
- `assert-*` take a `Block!` (not bare args) so the test body stays
  homoiconic and the asserted expressions aren't eagerly evaluated by the
  surrounding `test` block's dispatch. The block is `reduce`'d inside the
  native. (`assert [a b]` reduces the block to `[a-val b-val]` and compares.)
- `test`/`suite` bodies are *not* `reduce`'d — they're stored as `Series` and
  later dispatched via `dispatch_block` in the runner. This mirrors how
  `if`/`either` treat their block args (`natives/control.rs:24–56`).
- `before-test`/`after-test` register hooks on the *current* suite's frame;
  calling them at the top level (outside any `suite`) is a no-op with a
  warning to stderr (matches Red's tolerance of misplaced directives).
- `run-tests` is idempotent within a single `--test` invocation: a second
  call re-runs the registry (the registry is not cleared), but the
  auto-invocation in `--test` mode only fires once (the runner sets an
  `env.tests_run: bool` guard).
- `fail` accepts a `String!` or a `Block!` (molded into the message) — same
  shape as `cause-error`.

### Nesting example

```red
suite "math" [
    before-test [x: 10]
    test "adds" [assert-equal [x + 5 15]]
    test "subs" [assert-equal [x - 5 5]]
    suite "trig" [
        test "sin 0" [assert-equal [sin 0 0.0]]
    ]
]
test "top-level" [assert [1 + 1 = 2]]
```

- A `test` outside any `suite` registers at the top level (path `[top-level]`).
- Nested `suite` prefixes compose: `"math/trig/sin 0"`.
- `before-test` only affects the enclosing `suite` (and is inherited by
  nested suites); child `before-test` runs *after* parent `before-test`.
- `after-test` runs unconditionally per test, even on failure or crash.

## TAP-14 output

```
TAP version 14
1..3
ok 1 - math/adds
not ok 2 - math/subs
  ---
  message: expected 15, got 5
  found: 5
  expected: 15
  where: assert-equal
  ---
ok 3 - math/trig/sin 0
# tests: 3, passed: 2, failed: 1
```

- Line 1: `TAP version 14` (only if at least one test ran; if zero tests,
  print `1..0` and exit 0 with a `# no tests registered` directive).
- Line 2: `1..N` plan line.
- Per test: `ok N - <path>` or `not ok N - <path>`. On failure, a YAML-ish
  diagnostic block indented 2 spaces, fenced by `---`/`...` (TAP §2.5).
  Fields: `message`, `found`, `expected`, `where`, `suite` (optional, if
  nested), `# crash` directive (if the test errored outside an `assert-*`).
- Final summary line: `# tests: N, passed: P, failed: F`. On any failure,
  print `# failed: <comma-separated path list>` after the summary.
- Exit code: `0` if `F == 0`, else `1` (via `EvalError::Quit(1)` — but see
  milestone 73 for why we *don't* use `Quit` and instead return the count
  via a new `Env` field read by the runner).

### Diagnostic block field mapping

| `ErrorValue` field | TAP YAML field | Source |
|---|---|---|
| `message` | `message` | `assert-*` builds: `"expected <exp>, got <got>"` |
| `args[0]` | `found` | the actual value (molded) |
| `args[1]` | `expected` | the expected value (molded) |
| `where` | `where` | the asserting word (`'assert-equal`, `'assert`, …) |
| — | `# crash` | set if the test raised a non-`Raised('test)` error |

---

## Milestone 70 — `Env` registry + `TestDef`/`TestHooks`/`TestResult` types ✅ LANDED

The foundational milestone. Adds the `Env` fields and the supporting structs
that the natives (M71) and the runner (M72) will read/write. **No behavior
change yet** — the fields default to empty and nothing reads them.

### Files

- [ ] **Edit: `crates/red-core/src/value.rs`** — add three structs near
  `ModuleDef` (`value.rs:~415`):
  ```rust
  #[derive(Clone, Debug)]
  pub struct TestDef {
      pub name: String,
      pub path: Vec<Symbol>,     // suite path; empty = top level
      pub body: Series,          // the `test [...]` block, stored verbatim
      pub span: Span,
  }

  #[derive(Clone, Debug, Default)]
  pub struct TestHooks {
      pub before: Option<Series>,
      pub after: Option<Series>,
  }

  #[derive(Clone, Debug)]
  pub enum TestStatus { Pass, Fail, Crash }

  #[derive(Clone, Debug)]
  pub struct TestResult {
      pub path: String,           // "suite/name" joined
      pub status: TestStatus,
      pub message: Option<String>,
      pub found: Option<String>,   // molded
      pub expected: Option<String>,// molded
      pub where_word: Option<Symbol>,
  }
  ```
  No `Value` variant — tests live in `Env`, not as values. Derive `Clone`+
  `Debug` (mirrors `ClosureDef`/`ModuleDef`).
- [ ] **Edit: `crates/red-core/src/env.rs`** — add fields to `Env`
  (`env.rs:143`), initialized empty in `new_with_output` (`env.rs:285`) and
  `Env::new`:
  ```rust
  pub tests: Vec<TestDef>,
  pub current_suite: Vec<Symbol>,        // stack; empty = top level
  pub test_hooks: Vec<TestHooks>,        // stack; one frame per active `suite`
  pub test_results: Vec<TestResult>,
  pub tests_run: bool,                    // guard against double-run
  pub test_failed: usize,                 // count, for the CLI exit code
  ```
  `current_suite`/`test_hooks` are *stacks* (push on `suite` enter, pop on
  exit) so nested suites compose. `tests` is the flat registry (pushed by
  `test`); `test_results` is filled by `run_tests`. `tests_run`/`test_failed`
  are read by the runner (M72) to short-circuit auto-run and compute the
  exit code.

### Tests

- [ ] **Edit: `crates/red-core/src/env.rs`** — inline `#[test]` confirming
  `Env::new()` initializes all six fields to empty/zero/false.

---

## Milestone 71 — `test`/`suite`/hooks/`assert-*`/`fail` natives ✅ LANDED

Implements the language surface. The natives only *register* (for
`test`/`suite`/hooks) or *raise* (for `assert-*`/`fail`); the actual test
*execution* is M72's `run_tests_native`. Registered in `register_natives`
(`registry.rs:115`); no new `Instr` — all dispatch through existing
`Call`/`CallUser`.

### Files

- [ ] **New: `crates/red-eval/src/natives/test.rs`** — implementations. Each
  native follows the `args: &[Value], refs: &RefineArgs, env: &mut Env` shape
  (`natives/control.rs:24` for the template).
  - `test_native`: arity 2. Validate `args[0]` is `Value::String`, `args[1]`
    is `Value::Block`. Push `TestDef { name, path: env.current_suite.clone(),
    body, span }` to `env.tests`. Return `Value::None`. **Does not run the
    body.**
  - `suite_native`: arity 2. Validate same as `test`. Push name to
    `env.current_suite`, push fresh `TestHooks::default()` to
    `env.test_hooks`, `dispatch_block(&body, env)` (so nested `test`/
    `suite`/`before-test` calls register), then pop both stacks. Return
    `Value::None`. Errors inside the suite body propagate (a `suite` whose
    body errors during collection aborts the whole script — matching how
    top-level errors behave today).
  - `before_test_native` / `after_test_native`: arity 1 (Block!). If
    `env.test_hooks` is empty, print a warning to stderr and return `none`
    (top-level hook is a no-op). Otherwise set `hooks.before`/`hooks.after`
    on the top frame (overwriting any previous hook in the same frame —
    last-wins, matching Red's tolerance).
  - `assert_native`: arity 1 (Block!). `reduce` the block (via
    `reduce`-style dispatch — reuse `eval_native`'s `reduce` at
    `natives/eval.rs`), take the last value, check `truthy`. If falsy, raise
    `EvalError::Raised(Rc::new(ErrorValue::new_structed("assertion failed",
    None, Some(Symbol::new("test")), Vec::new(), None,
    Some(Symbol::new("assert")), None)))`. Return `Value::None` on pass.
  - `assert_equal_native` / `assert_not_equal_native`: arity 1 (Block! of 2
    values). `reduce` the block to `[a-val b-val]`. Compare via
    `values_equal` (`natives/compare.rs`, already `pub(crate)` via
    `natives/mod.rs:40`). On mismatch, raise `Raised` with `message:
    "expected <mold b>, got <mold a>"` (or the inverse for
    `assert-not-equal`), `args: [a-val, b-val]`, `where: 'assert-equal`.
  - `assert_error_native`: arity 1 (Block!). Run via the same `try`-wrap
    pattern as `try_native` (`natives/control.rs`): `dispatch_block` in a
    catch context. If the result is `Ok`, raise `Raised` with `message:
    "expected an error, but the block succeeded"`, `where: 'assert-error`.
    If `Err(Raised(_))` or `Err(Native{..})`, return `none` (pass). Other
    control-flow unwinds (`Break`/`Continue`/`Throw`/`Quit`/`Return`)
    propagate unchanged.
  - `assert_no_error_native`: the inverse — pass on `Ok`, fail on `Err`.
  - `fail_native`: arity 1 (String! or Block!). Raise `Raised` with the
    string (or molded block) as `message`, `where: 'fail`.
- [ ] **Edit: `crates/red-eval/src/natives/mod.rs`** — add `mod test;` to the
  module list (`mod.rs:32–38`) and `pub(crate) use test::register_test_natives;`
  alongside the existing `pub use registry::{install_constants, register_natives};`
  (`mod.rs:43`).
- [ ] **Edit: `crates/red-eval/src/natives/registry.rs`** — at the end of
  `register_natives` (`registry.rs:115`, after the last group), call
  `register_test_natives(env)` (or inline the `env.natives.insert(...)`
  calls — mirroring the I/O block at `registry.rs:117–122`). All test words
  are `fixed_native` with arity 0–2; no refinements.

### Tests

- [ ] **Edit: `crates/red-eval/src/natives/test.rs`** — `#[cfg(test)] mod tests`
  mirroring `natives/mod.rs:193–243` (`run_capture` helper, `BufferWriter`,
  fresh `Env` per test). Cases:
  - `test_registers_without_running` — `test "x" [print "ran"]` produces no
    stdout, `env.tests.len() == 1`.
  - `suite_nests_path_prefix` — `suite "a" [suite "b" [test "c" [...] ]]`
    registers with `path == ["a" "b"]`.
  - `before_test_runs_before_test` — confirmed in M72 (this milestone only
    checks the hook is *stored*; M72 checks execution order).
  - `assert_passes_on_truthy` — `assert [true]` returns `none`, no error.
  - `assert_fails_on_falsy` — `assert [false]` raises; `try` catches an
    `error!` with `type: 'test`.
  - `assert_equal_message_includes_molds` — `try [assert-equal [1 2]]`
    yields an error whose `message` contains both `"1"` and `"2"`.
  - `assert_error_passes_when_body_raises` —
    `assert-error [1 / 0]` returns `none`.
  - `assert_error_fails_when_body_succeeds` —
    `try [assert-error [1 + 1]]` yields a `test` error.
  - `fail_raises_with_message` — `try [fail "boom"]` yields `message: "boom"`.
  - `top_level_before_test_is_noop` — `before-test [x: 1]` at top level
    prints a warning to stderr, doesn't crash.

---

## Milestone 72 — `run-tests` native + TAP reporter + isolation ✅ LANDED

The runner. Drains `env.tests`, runs each in an isolated child `Context`,
composes `before-test`/`after-test` hooks from the suite stack, captures
results into `env.test_results`, prints TAP-14 to `env.out`, sets
`env.test_failed` and `env.tests_run`. **This is the only milestone that
actually executes test bodies.**

### Files

- [ ] **Edit: `crates/red-eval/src/natives/test.rs`** — add
  `run_tests_native` (arity 0):
  1. Guard: if `env.tests_run` is true, return `none` (idempotent).
  2. Set `env.tests_run = true`.
  3. Write `TAP version 14\n` and `1..N\n` to `env.out` (via
     `writeln!(env.out, ...)`; `env.out` is `Box<dyn Write>` —
     `env.rs:285`).
  4. For each `TestDef` in `env.tests` (index `i`, 1-based for TAP):
     a. Build the test's full hook chain: walk the suite path
        (`test.path`) and collect each frame's `before`/`after` from a
        fresh `test_hooks` snapshot taken at `suite` enter time. **Open
        question — see §"Hook composition" below**; resolution: store
        `hooks: TestHooks` *on the `TestDef` itself* at registration time
        (M71 `test_native` copies the current `test_hooks` stack into the
        `TestDef`). This avoids re-walking the suite stack at run time and
        makes hook composition explicit. Update M71's `TestDef` to include
        `pub hooks: Vec<TestHooks>` (the full inherited chain, parent-first).
     b. Create a fresh child `Context` of `user_ctx`
        (`Context::new_child(env.user_ctx)` — add a `new_child` constructor
        to `Context` if it doesn't exist; `use_native` does this via
        `Context::clone` + swap, but we want a true child for `unbound →
        parent` resolution. Check `context.rs:24–27` for whether `Context`
        has a parent link — if not, M72a adds one; if adding a parent link
        is invasive, fall back to `use`'s shallow-clone pattern and accept
        that unbound words in a test resolve only against the test's own
        context + `user_ctx` via the existing `LoadDynamic` fallback, which
        is sufficient since `user_ctx` is the test's parent in practice.).
     c. Swap `env.user_ctx` to the child (save the old `Rc<Context>` to
        restore after).
     d. Run `before` hooks in order (parent → child). Each `before` block
        is `dispatch_block`'d in the child context. If a `before` hook
        errors, mark the test as `Crash` with the error's message, skip
        the test body, still run `after` hooks (finally semantics).
     e. If no `before` hook errored: `dispatch_block(&test.body, env)`
        wrapped in a `try`-style catch. On `Ok`, status = `Pass`. On
        `Err(Raised(ev))` with `ev.type == 'test`, status = `Fail`,
        populate `message`/`found`/`expected`/`where_word` from the
        `ErrorValue`. On `Err` with any other `Raised` or `Native` or
        `TypeError`/`Arity`/`UnboundWord`/`Compile`, status = `Crash`,
        `message = e.to_string()`. Control-flow unwinds (`Return`/`Break`/
        `Continue`/`Throw`/`Quit`) propagate — `Throw` is a crash, `Quit`
        aborts the whole run (re-raise).
     f. Run `after` hooks in reverse order (child → parent), each in the
        same child context, wrapped in catch. An `after` hook error
        upgrades the test to `Crash` (if it was `Pass` or `Fail`) with
        `message: "after-test hook failed: <ev.message>"`. The test's
        original status is preserved in the `message` prefix.
     g. Restore `env.user_ctx`.
     h. Push `TestResult` to `env.test_results`. Increment
        `env.test_failed` if status != Pass.
     i. Write the TAP line for this test to `env.out`:
        - `ok N - <path>\n` on Pass.
        - `not ok N - <path>\n` + YAML diagnostic block on Fail/Crash.
  5. After the loop, write the summary line
     `# tests: N, passed: P, failed: F\n` and, if `F > 0`,
     `# failed: <comma-joined paths>\n`.
  6. Return `Value::None`. (The exit code is communicated via
     `env.test_failed`, read by M73's runner — *not* via `EvalError::Quit`,
     because `Quit` would prevent the summary line from being flushed if
     the runner is called as a normal native in non-`--test` mode.)
- [ ] **Edit: `crates/red-core/src/context.rs`** — (only if M72a is needed)
  add `Context::new_child(parent: Rc<Context>) -> Rc<Context>` that creates
  a child context with unbound-word fallback to `parent`. If this is too
  invasive (the existing `Context` has no parent link, `context.rs:24–27`),
  skip it and rely on the `LoadDynamic` → `user_ctx` fallback in the VM
  (`vm/lex.rs`) and the `resolve_word` fallback in the walker
  (`interp_walker.rs`). Tests that reference top-level words still work
  because the child is a shallow clone of `user_ctx` — SetWords in the
  test shadow, reads fall through to the cloned slots. Document the
  chosen approach in `test.rs` comments.

### Tests

- [ ] **Edit: `crates/red-eval/src/natives/test.rs`** — `#[cfg(test)]` cases:
  - `run_tests_passes_all` — 3 passing tests → TAP `1..3`, 3 `ok` lines,
    summary `# tests: 3, passed: 3, failed: 0`, `env.test_failed == 0`.
  - `run_tests_reports_failure` — 1 failing `assert-equal` → `not ok`,
    YAML block contains `found:`/`expected:`/`where: assert-equal`.
  - `run_tests_crash_vs_fail` — a test that does `1 / 0` (not via `assert`)
    → `not ok` with `# crash` directive, `where:` absent.
  - `before_test_runs_before_each_test` — `before-test [counter: 0]`,
    `test "a" [counter: counter + 1 assert-equal [counter 1]]`,
    `test "b" [counter: counter + 1 assert-equal [counter 1]]` — both
    pass (counter resets per test).
  - `after_test_runs_on_failure` — `after-test [print "cleanup"]`, a
    failing test → stdout contains `cleanup`.
  - `nested_suite_hook_inheritance` — parent `before-test [x: 1]`, child
    `before-test [x: x + 1]`, test asserts `x == 2` (child runs after
    parent).
  - `run_tests_isolates_setwords` — `test "a" [y: 99]`,
    `test "b" [assert [not value? 'y]]` — both pass (y doesn't leak).
  - `run_tests_idempotent` — calling `run-tests` twice produces TAP once
    (second call is a no-op, `env.tests_run` guard).
  - `run_tests_empty_registry` — `run-tests` with no tests → `1..0`,
    `# no tests registered`, exit 0.
  - `run_tests_zero_tests_exits_zero` — confirms `env.test_failed == 0`
    when the registry is empty.

---

## Milestone 73 — `RunOptions.test_mode` + CLI `--test` flag + exit code ✅ LANDED

Wires the runner into the CLI. After the script's top-level evaluation, if
`opts.test_mode` is set, auto-invoke `run_tests` (unless the script already
called `run-tests` explicitly — detected via `env.tests_run`). Exit code is
`env.test_failed > 0 ? 1 : 0`.

### Files

- [ ] **Edit: `crates/red-eval/src/interp_runner.rs:69`** — add
  `pub test_mode: bool` to `RunOptions` (default `false` via `#[derive(Default)]`).
- [ ] **Edit: `crates/red-eval/src/interp_runner.rs:213`** — after the
  `match dispatch_block(&block, &mut env)`, before constructing the return
  value, add:
  ```rust
  if opts.test_mode && !env.tests_run && !env.tests.is_empty() {
      // Auto-invoke run_tests. The native signature is (args, refs, env);
      // call directly with empty args/refs.
      let _ = crate::natives::test::run_tests_native(&[], &red_core::RefineArgs::default(), &mut env);
  }
  ```
  Then in the `Ok(v)` arm, set the exit code: if `opts.test_mode &&
  env.test_failed > 0`, return `Ok((Value::None, 1))`; otherwise `Ok((v, 0))`.
  In the `Err(EvalError::Quit(code))` arm, if `opts.test_mode`, still run
  the auto-test (a `quit` during collection shouldn't skip tests — but in
  practice a `quit` during collection means the script aborted, so tests
  weren't registered; in that case `env.tests.is_empty()` is true and the
  auto-run is skipped). Keep the existing `Ok((Value::None, code))` for the
  non-test case.
- [ ] **Edit: `crates/red-cli/src/main.rs:72`** — add `let mut test = false;`
  alongside `let mut walk = false;` (`main.rs:63`). Add a flag branch:
  `} else if a == "--test" { test = true;`. Pass `test` through to
  `run_file` (add it to the signature, mirroring `walk`/`trace`/`no_stdlib`).
  Set `opts.test_mode = test` in `run_file` (`main.rs:154–161`).
- [ ] **Edit: `crates/red-cli/src/main.rs:108`** — in the `--disasm`/
  `--disasm-func` precedence block, error out if `test` is also set:
  ```rust
  if (disasm || disasm_func.is_some()) && test {
      eprintln!("*** Error: --test and --disasm are mutually exclusive");
      return ExitCode::from(1);
  }
  ```
- [ ] **Edit: `crates/red-cli/src/main.rs:16` (HELP)** — add a line:
  ```
  red --test <file.red>                                          Run the test suites/tests declared in the file and report (TAP)
  ```
  and a paragraph in the help body explaining `--test`: collects `test`/
  `suite` declarations, runs them in isolation, prints TAP-14 to stdout,
  exits 0 on all-pass, 1 on any failure. Mutually exclusive with `--disasm`.

### Tests

- [ ] **Edit: `crates/red-cli/tests/cli.rs`** — `assert_cmd`-based cases:
  - `--test_passes_exit_zero` — a fixture with one passing test → stdout
    contains `ok 1`, exit code 0.
  - `--test_fails_exit_one` — a fixture with one failing `assert-equal` →
    stdout contains `not ok 1`, exit code 1.
  - `--test_no_tests_exit_zero` — a fixture with no `test` calls → stdout
    contains `1..0`, exit code 0.
  - `--test_with_disasm_errors` — `red --test --disasm foo.red` exits 1
    with the mutual-exclusion error.
  - `--test_with_walk_parity` — `red --test --walk foo.red` produces the
    same TAP as `red --test foo.red` (parity sanity; the full parity gate
    is `cargo test --workspace --features force-walk`).

---

## Milestone 74 — Example + README + docs ✅ LANDED

Documentation. No code changes.

### Files

- [ ] **New: `examples/tests.red`** — a demo script using `suite`/`test`/
  `assert-*`/hooks, runnable via `cargo run -p red-cli -- --test examples/tests.red`.
  Should include: a passing suite, a nested suite, a `before-test` hook, an
  `assert-error` case, and one deliberately-failing test (commented out by
  default with a note on how to uncomment to see the TAP failure output).
- [ ] **Edit: `README.md:44` (Build & run block)** — add a line:
  ```
  cargo run -p red-cli -- --test examples/tests.red        # run the unit tests declared in the file (TAP output)
  ```
- [ ] **Edit: `README.md` (What's implemented)** — add a "Unit testing"
  subsection under "Natives" listing the test words and the `--test` CLI
  mode. Cross-link to `examples/tests.red`.
- [ ] **Edit: `README.md:31` (Status / Workspace)** — bump the v0.5.0 line
  to mention `--test` mode (or note it as a v0.5.x patch — decide at release
  time; this plan doesn't bump the version).
- [ ] **New: this file (`plan10-unit-testing.md`)** — the plan you're reading.

---

## Open questions

### Hook composition

**Q:** Should `before-test`/`after-test` hooks be inherited by nested
suites, and in what order?

**A (resolved in M72):** Yes, inherited. Parent `before-test` runs first,
child `before-test` runs second (so the child sees the parent's setup).
`after-test` runs in reverse: child first, parent last. Hooks are *stored
on the `TestDef` at registration time* (M71 `test_native` copies the current
`test_hooks` stack — parent-first — into the `TestDef`), so the runner
doesn't need to re-walk the suite stack. This makes hook composition
explicit and avoids ordering ambiguity. **Update M71's `TestDef` to
include `pub hooks: Vec<TestHooks>` (the full inherited chain).**

### Isolation mechanism

**Q:** Should each test run in a true child `Context` (with parent fallback)
or a shallow clone of `user_ctx` (the `use` pattern)?

**A (resolved in M72):** Shallow clone (the `use` pattern at
`natives/words.rs:603–661`). Adding a parent link to `Context` is invasive
(the `Context` has no parent field, `context.rs:24–27`), and the existing
`LoadDynamic`/`resolve_word` fallbacks already resolve unbound words
through `user_ctx` — which *is* the test's effective parent since the clone
copies `user_ctx`'s slots. SetWords in the test shadow the cloned slots;
reads of words the test didn't set fall through to the cloned values (which
mirror `user_ctx`). Document this in `test.rs` comments. A true child
context with formal parent fallback is a v0.6 candidate (it would also
clean up `use`/`make object!` semantics).

### Exit code communication

**Q:** Should `run_tests_native` signal failure via `EvalError::Quit(1)` or
via an `Env` field?

**A (resolved in M72/M73):** `Env` field (`env.test_failed`). `Quit` would
prevent the TAP summary line from being flushed if `run-tests` is called as
a normal native in a non-`--test` script (the user might want to run tests
inline and then do more work). The runner (M73) reads `env.test_failed` after
the auto-invocation and sets the exit code there. If a user calls
`run-tests` explicitly in a non-`--test` script, the TAP is printed but the
exit code is 0 (the script continues) — matches the "tests are a normal Red
value" principle.

### `assert` argument shape

**Q:** Should `assert` take a `Block!` (eager-evaluated by `reduce` inside
the native) or a single expression value?

**A (resolved in M71):** `Block!`. Reasons: (1) homoiconicity — the test
body reads as data; (2) the block isn't eagerly evaluated by the
surrounding `test` block's dispatch (the `test` native stores the body
verbatim, so the `assert [...]` form survives until `run-tests` dispatches
the body); (3) `assert [1 + 1 = 2]` is more readable than
`assert (1 + 1 = 2)` (which would require paren-grouping anyway). The
block is `reduce`'d inside `assert_native` and the last value is tested
for truthiness. `assert-equal [a b]` reduces to two values and compares.

## Verification

1. `cargo build --workspace` — M70 `Env` additions don't break existing
   natives (all new fields default to empty).
2. `cargo test --workspace` — existing tests stay green (test natives are
   inert unless `test_mode`/`run-tests` is invoked; the registry is empty
   in all existing fixtures).
3. `cargo test --workspace --features force-walk` — parity (the new natives
   go through `dispatch_block`, which already routes Walk/VM; the parity
   harness `tests/parity.rs` exercises them automatically).
4. `cargo run -p red-cli -- --test examples/tests.red` — TAP output, exit 0
   on all-pass. Uncomment the deliberately-failing test, re-run, confirm
   exit 1 and the YAML diagnostic block.
5. New inline `#[test]`s in `natives/test.rs` (M71, M72) and CLI tests in
   `red-cli/tests/cli.rs` (M73).

## Risks

- **`Env` field bloat:** six new fields on a struct that's already
  ~20 fields. Mitigated: all are `Vec`/`bool`/`usize` with `Default`
  impls; zero cost when unused (the registry is empty in all existing
  tests).
- **Hook composition complexity:** the "store hooks on `TestDef` at
  registration time" decision (M71) is the linchpin. If it proves
  brittle, fall back to re-walking the suite stack at run time (slower,
  but the registry is small).
- **`Context` parent link:** avoided by using the shallow-clone pattern.
  If tests need true isolation (a SetWord in a test leaking to the next
  test's reads via shared `Rc<RefCell<...>>` series), a true child context
  is a v0.6 follow-up. The shallow-clone approach mirrors `use`, which is
  battle-tested.
- **TAP version drift:** the plan targets TAP-14 (the latest spec). If a
  consumer expects TAP-13, the `TAP version 14` line is forward-compatible
  (TAP parsers ignore unknown version lines). No risk.
- **`--test` + `--no-stdlib` interaction:** test natives are always-on, so
  `--test --no-stdlib` works; test bodies just can't use stdlib words
  (e.g. `str-upper`). Documented in `--test` help.
