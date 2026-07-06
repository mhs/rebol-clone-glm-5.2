# Plan 7: Package Manager & Native Plugins (v0.6)

Execution checklist extending the v0.5.0 baseline in `plan6-closures-modules.md`.
v0.6 lands **modern package management** — a `red pkg` toolchain, a
`package.red` manifest dialect, git/path source resolution, a `red.lock`
lockfile, and **runtime-loadable native plugins** via cdylib + `dlopen`.

Per `../../project-brief.md`, every new construct is additive through the existing
VM `Const`-pool + `Call`/`CallUser`/`MakeFunc` path; the v0.3.3 VM stays the
default evaluator. The parity harness (`tests/parity.rs`) and
`cargo test --workspace --features force-walk` remain the regression gates.

Deferred to v0.7+ (acknowledged, not built here): reactivity, concurrency,
`tag!`/`ref!`/`image!`/`vector!`/`hash!`/`regex!`, `routine!` FFI (the cdylib
plugin layer supersedes the `routine!` design — see "Relationship to
`routine!`" below), `compose`-on-closure, named timezones, shared-cell
closures, `unimport`, a central package registry server, plugin hot-reload,
plugin sandboxing (cdylib plugins run with full process privileges; sandboxing
would mean WASM, which is a deliberately deferred alternative), plugin unload
(UB with live `Rc`s), cross-compilation of native cdylibs (host arch only).

## Design summary

Four themes, in priority order:

1. **Runtime native plugins via cdylib + `dlopen` (M70)** — a package may
   include Rust code that compiles to a cdylib. The runtime `dlopen`s it and
   calls an `extern "C" fn red_register(env: *mut RedEnv) -> i32`. Plugins
   **never** see `Env`/`Value`/`FuncDef`/`NativeFn` directly; a new
   `red-plugin-abi` crate exposes opaque handles (`RedEnv`, `RedValue`,
   `RedArgs`) plus `extern "C"` accessor functions. `Value`'s internal
   layout can change freely between versions — only the accessor signatures
   are ABI-stable. Plugins are namespaced as synthetic `module!` values: a
   plugin's `red_register` populates a module ctx with native `FuncDef`s, and
   scripts access them via `import 'pkgname` + bare-word calls (reuses the
   M61–M62 module machinery verbatim — no new dispatch path).
2. **Drift protection — three independent layers (M72)** — Layer 1:
   `RED_ABI_VERSION` const exported by the plugin; **patch-ok, minor-exact**
   match required (a `0.5.0` plugin loads in a `0.5.3` host; refuses in
   `0.6.0`). Layer 2: `LAYOUT_HASH` const — a compile-time hash of every
   type crossing the boundary (using `abi_stable`-style layout IDs); catches
   silent layout drift within the compat window. Layer 3: the
   `red-plugin-abi` crate itself — opaque handles + `extern "C"` accessors
   with `#[repr(C)]` primitives; system allocator everywhere; ownership via
   `retain`/`release` on handles (never raw `Rc` across the boundary).
3. **`package.red` manifest + `red pkg` CLI (M71)** — a Red block dialect
   parsed by the existing `parse` dialect (zero new parser code — a `package`
   dialect native walks the block). Schema: `name`/`version`/`author`/
   `license`/`red-version`/`depends`/`native`/`lib` keyword pairs. Sources:
   git (`git:`/`tag:`/`rev:`/`branch:`) and local path (`path:`). `red.lock`
   (TOML) records resolved SHAs + content hashes. CLI subcommands:
   `init`/`add`/`install`/`build`/`run`, all dispatched inside `red-cli`
   (no new binary).
4. **Dependency resolution + polish (M73–M74)** — transitive `depends:`
   resolution via topological sort; cycle detection; `red-version` constraint
   conflict reporting. `load-plugin` native gated on `--allow-shell` (same
   threat model as `call`/`shell` — running native code = running a
   process). Error rendering, golden fixtures, docs.

Non-goals: WASM plugins (deferred alternative — see "Why not WASM" below); a
central registry server (git-only for v0.6; `red pkg publish` = `git tag
v1.2.0 && git push`); plugin sandboxing (cdylib plugins run with full
process privileges; sandboxing requires WASM); plugin hot-reload (needs
unload, which is UB with live `Rc`s); cross-compilation (host arch only).

## Ground-truth references (from research)

- `Env::natives: HashMap<Symbol, Rc<FuncDef>>` (`crates/red-core/src/env.rs:146`)
  — the registry plugins write into. Already runtime-extensible (no "only at
  startup" assumption in the type); `invalidate_native_index`
  (`env.rs:191–193`) is called when `natives` mutates, so plugin-loaded
  natives are visible to the VM's `NativeRegistry::from_env` snapshot on the
  next `dispatch_block`.
- `NativeFn = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>`
  (`crates/red-core/src/env.rs`) — a plain `fn` pointer, not a trait object.
  Plugin natives can't be `NativeFn` directly (different compilation unit,
  different crate version). The host-side `red_env_register_native` impl
  wraps the plugin's `extern "C" fn` in a Rust trampoline that marshals args
  through the ABI shim and stores it as a `FuncDef` with `native: Some(trampoline)`.
- `Env` is `!Send`, `Rc`/`RefCell`-laden (`env.rs:143–235`). Plugins get an
  opaque `*mut RedEnv` handle; they never touch `Rc` directly. Ownership of
  cross-boundary values is explicit (`retain`/`release` on handles).
- `--allow-shell` gating (`env.rs:151`, `main.rs:74`) — `call`/`shell` raise
  `EvalError::Native` unless `--allow-shell`. `load-plugin` is gated
  identically: running arbitrary native code = running a process.
- `module`/`export`/`import` machinery (`crates/red-eval/src/module.rs`,
  M61–M62) — a plugin's native `FuncDef`s are registered into a synthetic
  `Value::Module`'s `ctx`; the plugin's `red_register` calls
  `red_env_register_native(name, fn, arity)` for each, which the host
  translates into `module.ctx.set(name, Value::Func(fd))` + `exports.insert(name)`.
  Scripts `import 'pkgname` (M62) and call bare words. **No new value
  variant, no new dispatch path.**
- `parse` dialect (`crates/red-eval/src/parse.rs`) — `package.red` is a `Red
  [keyword: value …]` block parsed by `parse` with a small `package` dialect
  rule set. No new lexer/parser code; `package.red` round-trips through
  `mold`/`load` (property-tested for general Red values already).
- `run_source_with_exit_opts` / `RunOptions` (`interp_runner.rs`) — gains
  `plugin_paths: Vec<PathBuf>` and `auto_install: bool`. `red pkg run` sets
  both; plain `red <file.red>` is unchanged.
- `system/options` object (`registry.rs:386–404`) — gains a `plugins` field
  (a `block!` of loaded plugin module names) so scripts can introspect.
- `Span`/`Error` model — new plugin errors fold into `EvalError`:
  `PluginAbiMismatch { plugin, expected, found }`,
  `PluginLayoutMismatch { plugin, expected_hash, found_hash }`,
  `PluginMissingSymbol { plugin, symbol }`,
  `PluginLoadFailed { plugin, cause }`,
  `DependencyCycle { chain }`,
  `DependencyConflict { pkg, requested, found }`.
- `Symbol` = `Rc<str>` newtype (`value.rs:41`) — plugin native names are
  `Symbol`s; no new type.
- No `libloading` dep today; M70 adds it to `red-eval`.
- No `abi_stable` dep today; M72 adds it to `red-plugin-abi` (or hand-rolls
  the layout hash — see open-q #3).

---

## Why cdylib + `dlopen` (not the alternatives)

The user explicitly rejected build-time composition ("I don't want to have
to compile" at install time). Three runtime-loading options remained:

- **Option B — cdylib + `dlopen`** (chosen): plugin Rust compiles once (by
  the package author, or via `red pkg build` on the user's machine if
  building from source). Runtime `dlopen`s the `.dylib`/`.so`. Per-call cost
  is one FFI trampoline (nanoseconds). Allocator isolation via opaque
  handles + `retain`/`release`. ABI drift defended by three layers (M72).
- **Option C — WASM via `wasmtime`** (declined): safer (sandboxed,
  version-independent, hot-reloadable) but per-call serialization cost is
  real (microseconds), `wasmtime` adds ~5–10 MiB to the binary, and `Rc`-
  laden `Env` can't cross the wasm boundary either — so C still needs an
  opaque-handle shim *over* the wasm boundary, paying both costs. Deferred
  to v0.7+ if sandboxing becomes a requirement.
- **Option D — Rust not extensible** (declined): doesn't meet the
  "package might include Rust code" goal.

The make-or-break for Option B is ABI drift. Python (PEP 384 `abi3`) and
Node.js (N-API) solved this with a deliberate stable C ABI shim — the
discipline this plan follows (Layer 3: `red-plugin-abi`). Ruby/PHP/Lua took
the cheaper refuse-to-load-on-version-mismatch path (Layer 1) and accept
that plugins recompile per minor release. This plan ships both layers:
Layer 1 as a cheap backstop, Layer 3 as the real defense. Layer 2
(layout hash) catches the silent-drift gap between them.

## Relationship to `routine!` FFI

Red's `routine!` (deferred in `../../project-brief.md:280`) is a *compile-time*
FFI binding generator — you write `routine "int printf(char* fmt, ...)"`
inline and the compiler generates a binding. The cdylib plugin layer is a
*runtime* extension mechanism — packages ship pre-compiled `.dylib`s with
arbitrary Rust logic, not just `ccall` wrappers. They're complementary:
`routine!` is for calling into C libraries from a Red script; the plugin
layer is for *authoring Red natives in Rust*. `routine!` remains a deferred
v0.7+ candidate and is **not** blocked by this plan. A future `routine!`
implementation could itself be a plugin (compiled into a cdylib that
registers `routine` as a native).

## Why not WASM (deferred rationale)

Documented here so the decision isn't re-litigated without new information:

- **Per-call cost:** every `Value` serializes at the wasm boundary
  (bincode or a Red-specific binary codec). Microseconds, not nanoseconds.
  The VM's hot path is native calls — a 1000× slowdown on `+` would
  regress `fib 30` from 535ms to ~500s.
- **Binary size:** `wasmtime` adds ~5–10 MiB; the current binary is
  ~3.8 MiB. Roughly triples the binary for the sandboxing benefit.
- **No `Rc` across the boundary:** `Env` is `Rc<RefCell<…>>`-laden and
  `!Send`. WASM plugins get an opaque handle *anyway* — so C's shim design
  applies, just over a wasm boundary instead of an FFI boundary. You pay
  both the shim cost *and* the wasm serialization cost.
- **When to revisit:** if (a) a real sandboxing requirement surfaces
  (plugins from untrusted sources), or (b) `wasmtime` shrinks dramatically,
  or (c) the hot-path native calls are restructured to batch, WASM becomes
  viable. Until then, cdylib + `dlopen` is the right call.

---

## Milestone 70 — `red-plugin-abi` crate + host-side loader + PoC plugin

The foundational milestone. Adds the `red-plugin-abi` crate (opaque handles
+ `extern "C"` accessor declarations), the host-side implementation in
`crates/red-eval/src/plugin.rs`, the `load-plugin` native, and one
proof-of-concept plugin (`examples/plugins/add/`) demonstrating a native
function round-trips through the ABI shim. **No package manager yet** —
plugins are loaded by explicit path. M71 builds the package layer on top.

### Files

- [ ] **New: `crates/red-plugin-abi/Cargo.toml`** — `name = "red-plugin-abi"`,
      `version = "0.6.0"`, `edition = "2021"`. Zero deps. `crate-type =
      ["rlib"]` (plugins link it as a normal dep; it's not itself a cdylib).
      This crate's *major version* IS the ABI version (Layer 1's
      `RED_ABI_VERSION` is derived from it).
- [ ] **New: `crates/red-plugin-abi/src/lib.rs`** — opaque handle types,
      `RedTag` enum, accessor `extern "C"` declarations, and a
      `#[macro_export]` `red_abi_version!()` macro that emits the
      `RED_ABI_VERSION` const (a `&[u8]` like `b"red-plugin-abi 0.6.0"`).
      Schema:
      ```rust
      // Opaque handles — pointers to undefined structs (zero layout exposure).
      // Plugin never dereferences these; it passes them to accessors.
      #[repr(C)]
      pub struct RedEnv { _private: [u8; 0] }
      #[repr(C)]
      pub struct RedValue { _private: [u8; 0] }
      #[repr(C)]
      pub struct RedArgs { _private: [u8; 0] }

      // Stable tag enum for value inspection. #[repr(C)] so the discriminant
      // layout is fixed. New variants appended only at the end (never
      // reordered/removed) — this IS part of the ABI.
      #[repr(C)]
      #[derive(Clone, Copy, Debug, PartialEq, Eq)]
      pub enum RedTag {
          None, Logic, Integer, Float, String, Char, Binary,
          Block, Paren, Word, SetWord, GetWord, LitWord,
          File, Url, Refinement, Object, Error, Map, Pair,
          Tuple, Date, Bitset, Func, Closure, Module,
      }

      // Plugin entry — the only symbol the loader looks for at dlopen time.
      // Returns 0 on success, nonzero error code on failure.
      pub type RedRegisterFn = extern "C" fn(env: *mut RedEnv) -> i32;
      // Native function signature — plugin implements these, host calls them.
      pub type RedNativeFn = extern "C" fn(
          env: *mut RedEnv,
          args: *mut RedArgs,
          argc: usize,
      ) -> *mut RedValue;  // returns a retained RedValue (caller releases)

      // Accessors the plugin calls. Host provides these as extern "C"
      // exports (declared here, defined in red-eval/src/plugin.rs).
      // The plugin links red-plugin-abi for the declarations only.
      extern "C" {
          // Registration — called from red_register.
          pub fn red_env_register_native(
              env: *mut RedEnv,
              name: *const u8, name_len: usize,
              f: RedNativeFn,
              arity: usize,
          ) -> i32;
          pub fn red_env_register_constant(
              env: *mut RedEnv,
              name: *const u8, name_len: usize,
              value: *mut RedValue,
          ) -> i32;

          // Argument access (inside a native call).
          pub fn red_args_get(args: *mut RedArgs, idx: usize) -> *mut RedValue;
          pub fn red_args_count(args: *mut RedArgs) -> usize;

          // Value inspection.
          pub fn red_value_tag(v: *const RedValue) -> RedTag;
          pub fn red_value_as_integer(v: *const RedValue, out: *mut i64) -> i32;
          pub fn red_value_as_float(v: *const RedValue, out: *mut f64) -> i32;
          pub fn red_value_as_logic(v: *const RedValue, out: *mut bool) -> i32;
          pub fn red_value_as_string(
              v: *const RedValue,
              out_ptr: *mut *const u8, out_len: *mut usize,
          ) -> i32;
          pub fn red_value_as_char(v: *const RedValue, out: *mut u32) -> i32;

          // Ownership — bump refcount / drop. Cross-boundary values are
          // reference-counted by the host (backed by Rc); retain/release
          // mirror Rc::clone/drop. Plugin must release every value it
          // received (args) or constructed (make_*) unless it returns it.
          pub fn red_value_retain(v: *mut RedValue);
          pub fn red_value_release(v: *mut RedValue);

          // Value construction — returns a retained value (caller releases
          // or returns it).
          pub fn red_value_make_integer(env: *mut RedEnv, n: i64) -> *mut RedValue;
          pub fn red_value_make_float(env: *mut RedEnv, f: f64) -> *mut RedValue;
          pub fn red_value_make_logic(env: *mut RedEnv, b: bool) -> *mut RedValue;
          pub fn red_value_make_string(
              env: *mut RedEnv, s: *const u8, len: usize,
          ) -> *mut RedValue;
          pub fn red_value_make_none(env: *mut RedEnv) -> *mut RedValue;
          pub fn red_value_make_block(
              env: *mut RedEnv, cap: usize,
          ) -> *mut RedValue;
          pub fn red_value_block_append(
              env: *mut RedEnv, block: *mut RedValue, value: *mut RedValue,
          ) -> i32;

          // Error raising — returns 1, the trampoline converts to EvalError.
          pub fn red_env_raise_error(
              env: *mut RedEnv,
              msg: *const u8, msg_len: usize,
          ) -> i32;
      }

      // Convenience macro for emitting the ABI version const in the plugin.
      #[macro_export]
      macro_rules! red_abi_version {
          () => {
              #[no_mangle]
              pub static RED_ABI_VERSION: &[u8] =
                  concat!("red-plugin-abi ", env!("CARGO_PKG_VERSION")).as_bytes();
          };
      }
      ```
- [ ] **Edit: root `Cargo.toml`** — add `crates/red-plugin-abi` to workspace
      `members`. (No deps added to the workspace; `red-plugin-abi` is
      zero-dep.)
- [ ] **Edit: `crates/red-eval/Cargo.toml`** — add `libloading = "0.8"` dep.
- [ ] **New: `crates/red-eval/src/plugin.rs`** — host-side loader + accessor
      implementations. Top-level file (matches `module.rs`/`object.rs`).
      - [ ] `PluginHandle` struct holding the `libloading::Library` (kept
            alive in `env.plugins` for the process lifetime — no unload),
            the plugin's name, version, and registered module value.
      - [ ] `load_plugin(env: &mut Env, path: &Path, name: &str) ->
            Result<PluginHandle, EvalError>`:
            1. `libloading::Library::new(path)`.
            2. `get::<extern "C" fn() -> &'static [u8]>("RED_ABI_VERSION")` —
               Layer 1 check. Refuse if the const is missing or the version
               string doesn't satisfy the **patch-ok, minor-exact** rule
               (parse both as semver; require same major.minor; host's patch
               >= plugin's patch is allowed, plugin's patch > host's patch is
               refused — a plugin built against a newer patch may use APIs
               the host doesn't have).
            3. `get::<extern "C" fn() -> u64>("RED_LAYOUT_HASH")` — Layer 2
               check. (M72 populates this; M70 stubs it as `0` and skips the
               check — see M72 for the hash derivation.)
            4. `get::<RedRegisterFn>("red_register")` — Layer "missing symbol"
               check. Error if absent.
            5. Build a fresh `Context` for the plugin's module; push a
               `Rc<RefCell<ModuleDef>>` onto `env.module_stack`; swap
               `env.user_ctx` to the module's ctx (mirrors `module_native`
               in `module.rs:48`).
            6. Call `red_register(env_ptr)` — the plugin calls
               `red_env_register_native` etc., which the host implements
               (see below) to insert `FuncDef`s into the swapped `user_ctx`
               (= the module's ctx).
            7. Restore `env.user_ctx`; pop `env.module_stack`.
            8. Mark every word in the module's ctx as exported (plugins
               register only public natives; there's no private/public
               distinction at the ABI level). Build a `Value::Module` and
               insert it into `env.modules[name]` (M61 cache) so
               `import 'name` works immediately.
            9. Hold the `Library` in `env.plugins`.
      - [ ] Host-side accessor implementations (`extern "C" fn`):
            - [ ] `red_env_register_native` — wraps the plugin's
                  `RedNativeFn` in a Rust trampoline closure stored as
                  `FuncDef.native: Some(trampoline)`. The trampoline: reads
                  args from `&[Value]`, builds a `RedArgs` vec of
                  `*mut RedValue` (retaining each), calls the plugin's
                  `extern "C" fn`, converts the returned `*mut RedValue`
                  back to `Value` (release the handle), releases the arg
                  handles, maps `NULL`/error to `EvalError::Native`.
                  Registers the `FuncDef` into the current module's ctx
                  (the swapped `env.user_ctx`) via `ctx.set(name, Value::Func(fd))`.
            - [ ] `red_env_register_constant` — `ctx.set(name, value)`.
            - [ ] `red_args_get` / `red_args_count` — index into the
                  `Vec<*mut RedValue>` the trampoline built.
            - [ ] `red_value_tag` — `match Value { … }` → `RedTag`.
            - [ ] `red_value_as_integer`/`_float`/`_logic`/`_string`/`_char` —
                  extract via `match`, write to `*mut T`, return 0 on
                  success / 1 on type mismatch. String accessor writes
                  `*const u8` + `len` (the `Rc<str>`'s bytes; the handle's
                  retain count keeps the `Rc` alive).
            - [ ] `red_value_retain`/`_release` — the `RedValue` handle is
                  a `Box<Rc<Value>>` (heap pointer to an `Rc`). `retain` =
                  `Rc::clone(&*boxed)`; `release` = drop the boxed `Rc`. When
                  the last `Rc` drops, the `Value` frees normally.
            - [ ] `red_value_make_*` — construct a `Value`, wrap in `Rc`,
                  `Box::new`, return as `*mut RedValue`.
            - [ ] `red_value_make_block` + `red_value_block_append` —
                  construct/append to a `Value::Block`'s `Series`.
            - [ ] `red_env_raise_error` — set a thread-local error string;
                  the trampoline reads it after the plugin returns `NULL`
                  and converts to `EvalError::Native`.
      - [ ] `register_plugin_natives(env: &mut Env)` — registers the
            `load-plugin` native (arity 1: file path; gated on
            `env.allow_shell`, mirroring `call`/`shell` in `io.rs`).
- [ ] **Edit: `crates/red-core/src/env.rs`** — add to `Env`:
      ```rust
      pub plugins: Vec<libloading::Library>,   // kept alive for process lifetime
      pub plugin_handles: Vec<PluginHandle>,   // for introspection / `system/options/plugins`
      ```
      (Forward-declare `PluginHandle` in `env.rs` or move to `value.rs` —
      decision: `value.rs` next to `ModuleDef`, since it's a value-shaped
      handle. Re-export from `lib.rs`.) Add `invalidate_native_index` call
      after `load_plugin` finishes (the VM's `natives_by_idx` snapshot is
      stale once a plugin registers natives — mirrors the existing
      invalidation discipline at `env.rs:191`).
- [ ] **Edit: `crates/red-eval/src/natives/registry.rs`** —
      `pub mod plugin;`, call `crate::plugin::register_plugin_natives(env)`
      alongside `crate::module::register_module_natives`.
- [ ] **Edit: `crates/red-eval/src/lib.rs`** — `pub mod plugin;`.
- [ ] **Edit: `crates/red-cli/src/main.rs`** — extend `--allow-shell` help
      text to mention `load-plugin` is also gated.
- [ ] **Edit: `crates/red-eval/src/natives/mod.rs`** — `type_name` for
      plugin module values (already covered by `Value::Module` arm; no
      change needed unless a new error variant requires it).

### New `EvalError` variants (M70)

- [ ] **Edit: `crates/red-core/src/env.rs`** (`EvalError` enum) — add:
      - `PluginAbiMismatch { plugin: String, expected: String, found: String, span: Span }`
      - `PluginMissingSymbol { plugin: String, symbol: &'static str, span: Span }`
      - `PluginLoadFailed { plugin: String, cause: String, span: Span }`
- [ ] **Edit: `crates/red-core/src/error.rs`** — `render_error` arms for
      the new variants: `*** Error: [loc: ]plugin error: <plugin>: ABI
      version mismatch (host expects <expected>, plugin provides <found>)`
      etc.

### Proof-of-concept plugin: `examples/plugins/add/`

A minimal cdylib plugin exposing one native `add [a b]` that returns `a + b`.
Demonstrates the full round-trip: plugin registers a native, host loads it,
script calls it.

- [ ] **New: `examples/plugins/add/Cargo.toml`** — `name = "red-plugin-add"`,
      `crate-type = ["cdylib"]`, `red-plugin-abi = { path = "../../../crates/red-plugin-abi" }`.
- [ ] **New: `examples/plugins/add/src/lib.rs`**:
      ```rust
      use red_plugin_abi::*;

      red_abi_version!();  // emits RED_ABI_VERSION const

      #[no_mangle]
      pub extern "C" fn red_register(env: *mut RedEnv) -> i32 {
          unsafe {
              let name = b"add";
              red_env_register_native(
                  env, name.as_ptr(), name.len(), add_native, 2,
              );
          }
          0
      }

      extern "C" fn add_native(env: *mut RedEnv, args: *mut RedArgs, _argc: usize) -> *mut RedValue {
          unsafe {
              let a = red_args_get(args, 0);
              let b = red_args_get(args, 1);
              let mut ai: i64 = 0;
              let mut bi: i64 = 0;
              if red_value_as_integer(a, &mut ai) != 0
                 || red_value_as_integer(b, &mut bi) != 0 {
                  red_env_raise_error(env, b"add: both args must be integer".as_ref());
                  return std::ptr::null_mut();
              }
              red_value_make_integer(env, ai + bi)
              // a, b are released by the trampoline; the returned value is
              // released by the trampoline after conversion to Value.
          }
      }
      ```
- [ ] **New: `examples/plugins/add/test.red`** — a script that calls the
      plugin. (M70 loads plugins by explicit path, so this script is the
      manual smoke test; M71 wires `red pkg run` to do it automatically.)
      ```red
      Red []
      load-plugin %examples/plugins/add/target/release/libred_plugin_add.dylib
      import 'add
      print add 3 4   ; => 7
      ```

### Tests (M70)

- [ ] Inline `#[test]`: `load_plugin` of a valid cdylib returns a
      `PluginHandle`; `import 'name` then `name/foo` works.
- [ ] Inline `#[test]`: `load_plugin` of a non-cdylib file errors with
      `PluginLoadFailed`.
- [ ] Inline `#[test]`: `load_plugin` of a cdylib missing `red_register`
      errors with `PluginMissingSymbol`.
- [ ] Inline `#[test]`: `load_plugin` of a cdylib with mismatched
      `RED_ABI_VERSION` errors with `PluginAbiMismatch`. (Build a second
      tiny plugin with a hand-edited version string, or stub the check
      with a test-only env flag.)
- [ ] Inline `#[test]`: `load_plugin` gated on `--allow-shell`
      (`env.allow_shell = false` → `EvalError::Native "shell disabled"`,
      mirroring `call`/`shell`).
- [ ] Inline `#[test]`: plugin native raises an error via
      `red_env_raise_error`; the trampoline converts to
      `EvalError::Native` with the message.
- [ ] Inline `#[test]`: plugin native returns a constructed `Value::Block`
      (exercises `red_value_make_block` + `red_value_block_append`).
- [ ] Inline `#[test]`: refcount discipline — calling a plugin native in a
      tight loop (1000×) doesn't leak (the trampoline releases all handles).
      Wrap in a `#[cfg(debug_assertions)]` leak check or count `Rc` strong
      counts before/after.
- [ ] `cargo test --workspace` green; `--features force-walk` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --all --check` clean.
- [ ] Manual: `cargo build --release` in `examples/plugins/add/`, then
      `cargo run -p red-cli -- --allow-shell examples/plugins/add/test.red`
      prints `7`.

### M70 open questions

1. **`PluginHandle` location.** `env.rs` or `value.rs`? Recommendation:
   `value.rs` (next to `ModuleDef`) so `env.rs` only holds the `Vec`, not
   the type definition. Confirm.
2. **Trampoline error path.** Thread-local error string vs. an out-param on
   every accessor? Recommendation: thread-local (`thread_local!` in
   `plugin.rs`) — simpler API, single-threaded runtime so no contention.
   Confirm.
3. **`RedArgs` representation.** `Vec<*mut RedValue>` or a fixed-size array?
   Recommendation: `Vec` (arity is unbounded in Red — `function` allows
   variadic via `Function`); the trampoline allocates one per call. Profile
   in M72 if it shows up. Confirm.

---

## Milestone 71 — `package.red` manifest + `red pkg` CLI + git/path sources

Builds the package manager on top of M70's plugin loader. Packages are
directories with a `package.red` manifest + `lib/` (Red source) + optional
`native/` (Rust cdylib crate). Sources: git and local path. `red.lock`
records resolved SHAs.

### Files

- [ ] **New: `crates/red-pkg/Cargo.toml`** — `name = "red-pkg"`, `version =
      "0.6.0"`. Deps: `red-core` (for `Value`/`parse`-based manifest
      parsing), `toml` (for `red.lock`), `serde`/`serde_derive`. Library
      crate (the CLI calls into it).
- [ ] **Edit: root `Cargo.toml`** — add `crates/red-pkg` to workspace
      `members`.
- [ ] **New: `crates/red-pkg/src/lib.rs`** — public API:
      - `pub fn parse_manifest(path: &Path) -> Result<Manifest, PkgError>`
        — reads `package.red`, lexes + parses with `red-core` (no `red-eval`
        dep — manifest parsing is a pure `red-core` lex+parse+mold
        operation; the `package` dialect walker is a small Rust match, not
        a `parse`-dialect rule, to keep `red-pkg` decoupled from `red-eval`'s
        runtime).
      - `pub fn resolve_deps(manifest: &Manifest, cache_dir: &Path) -> Result<DepGraph, PkgError>`
        — topological sort over `depends:`; git fetch via `std::process::Command`;
        cycle detection.
      - `pub fn install(manifest: &Manifest, cache_dir: &Path) -> Result<InstallPlan, PkgError>`
        — fetches all deps into `~/.red/cache/<pkg>-<version>/`; for each
        dep with a `native:` dir, `cargo build --release` in that dir (shells
        out); copies the cdylib to `~/.red/cache/native/<pkg>-<version>/`.
      - `pub fn build_native(manifest: &Manifest) -> Result<PathBuf, PkgError>`
        — builds the current package's `native/` cdylib.
      - `pub fn run_with_deps(file: &Path, opts: RunOpts) -> Result<i32, PkgError>`
        — installs deps if missing, sets up `--module-path`, invokes
        `red-cli`'s run path with `load-plugin` for each native dep.
      - `pub fn write_lockfile(plan: &InstallPlan, path: &Path) -> Result<(), PkgError>`
        — serializes resolved SHAs + content hashes to `red.lock` (TOML).
      - `pub fn read_lockfile(path: &Path) -> Result<InstallPlan, PkgError>`
        — deserializes; used by `install` to skip re-resolution when
        `red.lock` is up-to-date.
- [ ] **New: `crates/red-pkg/src/manifest.rs`** — `Manifest` struct:
      ```rust
      pub struct Manifest {
          pub name: String,
          pub version: String,           // semver
          pub author: Option<String>,
          pub license: Option<String>,
          pub red_version: String,       // "0.5" — minor-exact constraint
          pub depends: Vec<Dep>,
          pub native: Option<PathBuf>,   // %native/ dir, optional
          pub lib: PathBuf,              // %lib/ dir, required
      }
      pub enum Dep {
          Git { name: String, url: String, tag: Option<String>,
                 rev: Option<String>, branch: Option<String> },
          Path { name: String, path: PathBuf },
      }
      ```
      Parser walks the parsed `Red [keyword: value …]` block:
      `name:`/`version:`/`author:`/`license:`/`red-version:` → string fields;
      `lib:`/`native:` → `file!` → `PathBuf`; `depends:` → block of
      `[name string! name string! …]` pairs or `[name path! …]` for path deps.
- [ ] **New: `crates/red-pkg/src/resolver.rs`** — `DepGraph`, topological
      sort, cycle detection (`DependencyCycle { chain: Vec<String> }`).
- [ ] **New: `crates/red-pkg/src/installer.rs`** — `InstallPlan`,
      fetch/build/copy logic, `red.lock` read/write.
- [ ] **New: `crates/red-pkg/src/error.rs`** — `PkgError` enum:
      `ManifestParse`, `DependencyCycle`, `DependencyConflict`,
      `GitFetchFailed`, `BuildFailed`, `LockfileCorrupt`, etc. `Display`
      impl prefixes `*** pkg error: `.
- [ ] **Edit: `crates/red-cli/src/main.rs`** — add `pkg` subcommand
      dispatch. If `args[0] == "pkg"`, route to `red_pkg::run_subcommand(&args[1..])`
      and exit (don't fall through to the script-run path). Subcommands:
      - [ ] `red pkg init [name]` — scaffold: write `package.red` template
            + `lib/` + optional `native/` (with `Cargo.toml` cdylib setup).
      - [ ] `red pkg add <name> [source]` — add to `depends:`, fetch, update
            `red.lock`. `source` is a git URL (`git+https://...`), a local
            path (`path:./../foo`), or a `name@version` shorthand for git
            tags.
      - [ ] `red pkg install` — read `package.red`, resolve via `red.lock`
            if present, fetch+build all deps into `~/.red/cache/`.
      - [ ] `red pkg build` — build the current package's `native/` cdylib
            (if any). Smoke-test via `load-plugin` against a temp script.
      - [ ] `red pkg run <file.red>` — `install` (if `red.lock` stale or
            missing), then run `<file.red>` with `--module-path
            ~/.red/cache/` + `load-plugin` for each native dep, then eval.
- [ ] **Edit: `crates/red-eval/src/interp_runner.rs`** — `RunOptions` gains
      `plugin_paths: Vec<PathBuf>` and `auto_load_plugins: bool`. When
      `auto_load_plugins`, `run_series_inner_opts` `load-plugin`s each path
      before evaluating the body. Plain `red <file.red>` sets
      `auto_load_plugins: false` (unchanged behavior).
- [ ] **Edit: `crates/red-eval/src/natives/registry.rs`** —
      `install_system` adds a `plugins: []` block to `system/options` (a
      `block!` of plugin module names, populated as `load-plugin` runs).

### `package.red` schema (concrete)

```red
Red [
    name: "mathutils"
    version: "1.2.0"
    author: "Jane Doe"
    license: "Apache-2.0"
    red-version: "0.5"                ; minor-exact constraint (Layer 1)
    depends: [
        strings "1.0"                  ; name + version (git tag v1.0)
        tree git: "https://github.com/foo/tree" tag: "v0.3"
        helper path: %../helper        ; local path dep
    ]
    native: %native/                   ; optional: dir with cdylib Cargo.toml + src/
    lib: %lib/                         ; required: dir with .red source files
]
```

- All fields except `name`/`version`/`lib` are optional; `init` template
  includes them commented-out.
- `depends:` entries: `name string!` (shorthand: name + version, fetches
  from a default git URL convention `https://github.com/red-pkg/<name>`,
  tag `v<version>`); `name git: <url> tag: <tag>`; `name git: <url> rev:
  <sha>`; `name git: <url> branch: <branch>`; `name path: %<path>`.
- `red-version: "0.5"` — the package requires a host with `red-core` 0.5.x.
  `install` checks this against the running binary's version; refuses on
  minor mismatch (Layer 1, but at the *package* level, distinct from the
  plugin's `RED_ABI_VERSION`).
- Round-trips through `mold`/`load` (the manifest is a normal Red block).

### `red.lock` schema (TOML)

```toml
# Auto-generated by `red pkg install`. Commit this.
[[package]]
name = "strings"
version = "1.0"
source = { git = "https://github.com/red-pkg/strings", rev = "abc123..." }
sha256 = "def456..."

[[package]]
name = "tree"
version = "0.3"
source = { git = "https://github.com/foo/tree", tag = "v0.3", rev = "789abc..." }
sha256 = "012abc..."

[[package]]
name = "helper"
version = "0.1.0"  # from path dep's package.red
source = { path = "../helper" }
```

- `rev` for git deps is the resolved commit SHA (pinned, regardless of
  whether `tag:` or `branch:` was specified). Branch deps resolve to the
  tip at install time; `red.lock` pins the SHA for reproducibility.
- `sha256` is a content hash of the fetched tree (detection of upstream
  rewrites — a tag force-pushed to a different commit).
- `red pkg install --locked` refuses to update `red.lock` (CI mode — fails
  if the lockfile is out of date).

### End-to-end example (M71 deliverable)

- [ ] **New: `examples/packages/strings/`** — a Red-only package (no Rust):
      `package.red` + `lib/strings.red` exporting `upper`/`lower`/`split-words`.
- [ ] **New: `examples/packages/mathutils/`** — a package with both Red
      and Rust: `package.red` (depends on `strings`), `lib/math.red`
      (Red `factorial`/`gcd`), `native/` (Rust `fast-fib` cdylib).
- [ ] **New: `examples/packages/mathutils/lib/main.red`** — a script that
      `import 'strings`, `import 'mathutils`, calls `strings/upper "hi"`,
      `factorial 5`, `fast-fib 30`.
- [ ] **Edit: `crates/red-cli/tests/cli.rs`** — integration test:
      `red pkg run examples/packages/mathutils/lib/main.red` exits 0 and
      prints expected output. (Requires network for the git dep — mark
      `#[ignore]` if the example uses a real git URL, or use a path dep in
      the example to stay hermetic. Decision: path dep for the test, git
      dep documented in `examples/packages/mathutils/package.red.example`.)

### Tests (M71)

- [ ] Inline `#[test]`: `parse_manifest` of a valid `package.red` returns
      the expected `Manifest`.
- [ ] Inline `#[test]`: `parse_manifest` of a malformed `package.red`
      errors with `ManifestParse`.
- [ ] Inline `#[test]`: `package.red` round-trips through `mold`/`load`
      (parse the block, mold it, parse again, compare).
- [ ] Inline `#[test]`: `resolve_deps` of a trivial graph (no deps) returns
      an empty `DepGraph`.
- [ ] Inline `#[test]`: `resolve_deps` detects a cycle (`a→b→a`).
- [ ] Inline `#[test]`: `install` of a path-dep-only package fetches
      (copies) into the cache and builds the native cdylib.
- [ ] Inline `#[test]`: `red.lock` written by `install` is readable by
      `read_lockfile` and produces an equivalent `InstallPlan`.
- [ ] Inline `#[test]`: `red pkg run` of the path-dep example exits 0.
- [ ] `cargo test --workspace` green; `--features force-walk` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --all --check` clean.

### M71 open questions

1. **Default git URL convention.** `name "1.0"` shorthand — fetch from
   where? Options: (a) `https://github.com/red-pkg/<name>` (a real
   org, requires setting one up); (b) no default — `name "1.0"` is
   invalid, must be `name git: <url> tag: "v1.0"`. Recommendation: (b)
   for v0.6 (no registry, no default URL); revisit when a registry
   exists. Confirm.
2. **Cache location.** `~/.red/cache/` (user-global) or `./red_cache/`
   (project-local)? Recommendation: `~/.red/cache/` (matches
   `~/.cargo`/`~/.npm`), with `--cache-dir` override. Confirm.
3. **`red.lock` format.** TOML (chosen) vs. a Red block (`red.lock.red`)?
   Recommendation: TOML — tooling-friendly, `serde` out of the box, and
   `red.lock` is *not* user-authored (unlike `package.red`). Confirm.
4. **`init` template scope.** Just `package.red` + `lib/` dirs, or also a
   `main.red` hello-world? Recommendation: include `lib/main.red` with
   `Red [] print "hello"` so `red pkg run lib/main.red` works immediately.
   Confirm.

---

## Milestone 72 — Drift protection layers 1 & 2

Populates the `RED_ABI_VERSION` and `RED_LAYOUT_HASH` consts that M70
stubbed/skipped, and wires the full drift-defense checks into `load_plugin`.

### Files

- [ ] **Edit: `crates/red-plugin-abi/src/lib.rs`** — the `red_abi_version!()`
      macro (M70) already emits `RED_ABI_VERSION`. Add a
      `red_layout_hash!()` macro that emits `RED_LAYOUT_HASH: u64`:
      ```rust
      #[macro_export]
      macro_rules! red_layout_hash {
          () => {
              // Compile-time hash of the layouts of every type crossing
              // the boundary. Computed by `abi_stable`'s
              // `GetLayoutEquivalent` derive, or hand-rolled from
              // `std::mem::size_of`/`align_of` of each #[repr(C)] type.
              // See open-q #3 for the derivation method.
              #[no_mangle]
              pub static RED_LAYOUT_HASH: u64 = /* computed const */;
          };
      }
      ```
- [ ] **Edit: `crates/red-eval/src/plugin.rs`** `load_plugin` — implement
      the full Layer 1 check:
      - Parse `RED_ABI_VERSION` as `&'static [u8]` → `&str` → split on
        space → second field is the semver.
      - Parse the host's `red-plugin-abi` version (compile-time const from
        `env!("CARGO_PKG_VERSION")`).
      - **Patch-ok, minor-exact:** `host.major == plugin.major &&
        host.minor == plugin.minor && host.patch >= plugin.patch`. Plugin
        built against a newer patch than the host → refuse (may use APIs
        the host lacks). Plugin built against an older patch → allow.
      - Refuse with `PluginAbiMismatch { expected: host_version, found:
        plugin_version }` on failure.
- [ ] **Edit: `crates/red-eval/src/plugin.rs`** `load_plugin` — implement
      the full Layer 2 check:
      - Read `RED_LAYOUT_HASH: u64` from the plugin.
      - Compare against the host's `RED_LAYOUT_HASH` (a const computed in
        `red-plugin-abi` at build time from the same layout inputs).
      - Refuse with `PluginLayoutMismatch { expected_hash, found_hash }` on
        mismatch. (New `EvalError` variant — add to `env.rs`.)
- [ ] **Edit: `crates/red-eval/src/plugin.rs`** `load_plugin` — Layer 3 is
      the existing `red-plugin-abi` shim (M70); no additional wiring, but
      document in `plugin.rs` module docs that the shim *is* Layer 3 and
      why it matters (opaque handles prevent the plugin from depending on
      `Value`'s field layout).

### Layout hash derivation (open-q #3)

Three options for computing `RED_LAYOUT_HASH`:

- **(a) `abi_stable` crate** — `#[derive(GetLayoutEquivalent)]` on each
  `#[repr(C)]` type; `assert_layouts!` macro computes a compile-time hash.
  Most robust; adds a dep to `red-plugin-abi`. Recommendation.
- **(b) Hand-rolled** — `const fn` hashing `size_of`/`align_of` of each
  `#[repr(C)]` type + the `RedTag` discriminant. Zero deps, but manual
  maintenance (every new type/accessor must be added to the hash input).
- **(c) No Layer 2** — rely on Layer 1 (minor-exact version match) alone.
  Simplest, but a within-minor layout change (e.g. reordering `Value`
  fields in a patch release) would silently break — Layer 3's opaque
  handles protect against this *for the plugin*, but the hash is a
  defense-in-depth signal. Recommendation: (a).

### Tests (M72)

- [ ] Inline `#[test]`: a plugin built against the current `red-plugin-abi`
      loads successfully (Layer 1 + Layer 2 pass).
- [ ] Inline `#[test]`: a plugin with a hand-edited `RED_ABI_VERSION`
      string (older minor) is refused with `PluginAbiMismatch`. (Build a
      second tiny cdylib with `RED_ABI_VERSION = b"red-plugin-abi 0.5.0"`.)
- [ ] Inline `#[test]`: a plugin with a hand-edited `RED_LAYOUT_HASH`
      (different `u64`) is refused with `PluginLayoutMismatch`.
- [ ] Inline `#[test]`: a plugin built against an older patch
      (`0.6.0` plugin in a `0.6.3` host) loads (Layer 1 allows).
- [ ] Inline `#[test]`: a plugin built against a newer patch (`0.6.3`
      plugin in a `0.6.0` host) is refused (Layer 1 rejects).
- [ ] `cargo test --workspace` green; `--features force-walk` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --all --check` clean.

---

## Milestone 73 — Transitive deps + conflict detection + `red-version` constraints

Fleshes out the dependency resolver to handle real-world graphs: transitive
deps, version conflicts, `red-version` constraints.

### Files

- [ ] **Edit: `crates/red-pkg/src/resolver.rs`** — extend `resolve_deps`:
      - [ ] Transitive resolution: for each dep, recursively read its
            `package.red` and resolve its `depends:`. Build a `DepGraph`
            (DAG) keyed by package name.
      - [ ] Cycle detection: DFS; on back-edge, return
            `DependencyCycle { chain: Vec<String> }` (the path from the
            cycle root back to itself).
      - [ ] Version conflict detection: if two packages depend on the same
            name with different versions, return
            `DependencyConflict { pkg, requested, found }`. (v0.6: refuse
            all conflicts — no version unification. v0.7+ may allow
            multiple versions if the package is a leaf with no shared
            state. See open-q #1.)
      - [ ] `red-version` constraint checking: for each package in the
            graph, its `red-version:` field is a minor-exact constraint
            (`"0.5"`). The host's `red-core` version must satisfy *every*
            package's constraint. Refuse with `RedVersionConflict {
            pkg, requested, host }` on mismatch.
- [ ] **Edit: `crates/red-pkg/src/installer.rs`** — `install` now:
      - [ ] Resolves the full transitive graph before fetching.
      - [ ] Fetches in topological order (deps before dependents).
      - [ ] For each package with a `native:` dir, `cargo build --release`
            in that dir, copy the cdylib to
            `~/.red/cache/native/<pkg>-<version>/`.
      - [ ] Writes the resolved graph (every package + its source + SHA +
            content hash) to `red.lock`.
      - [ ] On re-`install` with a matching `red.lock`, skips fetching/
            building (cached). `--force` re-fetches.

### Conflict resolution policy (v0.6)

- **Same name, different version → refuse.** No semver unification, no
  multiple-versions-side-by-side. The user must pin compatible versions in
  their `package.red`. Rationale: Red's module system (M61) caches named
  modules by name; two versions of the same name would clobber. v0.7+ may
  allow side-by-side via namespaced module paths (`pkg@1.0/foo`).
- **`red-version` conflict → refuse.** If package A needs `red 0.5` and
  package B needs `red 0.6`, the host can't satisfy both. Clear error
  naming both packages and their constraints.

### Tests (M73)

- [ ] Inline `#[test]`: a 3-level transitive graph (`a→b→c`) resolves and
      installs in topological order.
- [ ] Inline `#[test]`: a cycle (`a→b→a`) is detected; `DependencyCycle`
      names the chain.
- [ ] Inline `#[test]`: a version conflict (`a needs strings "1.0", b needs
      strings "2.0"`) is detected; `DependencyConflict` names both.
- [ ] Inline `#[test]`: a `red-version` conflict (`a needs "0.5", b needs
      "0.6"`) is detected; `RedVersionConflict` names both.
- [ ] Inline `#[test]`: re-`install` with a matching `red.lock` skips
      fetching (verify via a side-effect counter — e.g. a sentinel file
      written by the fetch step).
- [ ] Inline `#[test]`: `--force` re-fetches even with a matching lock.
- [ ] `cargo test --workspace` green; `--features force-walk` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `cargo fmt --all --check` clean.

### M73 open questions

1. **Multiple versions side-by-side.** v0.6 refuses. v0.7+ may allow via
   namespaced module paths (`strings@1.0/foo` vs `strings@2.0/foo`),
   requiring the M61 module cache to key by `(name, version)` rather than
   `name`. Document as a v0.7 candidate. Confirm defer.
2. **`cargo` invocation.** `std::process::Command::new("cargo")` — what if
   `cargo` isn't on PATH? Recommendation: clear error
   `*** pkg error: cargo not found on PATH (required to build native deps)`.
   Confirm.
3. **Offline mode.** `red pkg install --offline` — refuse to fetch, use
   only `~/.red/cache/` + `red.lock` SHAs? Recommendation: yes, useful for
   CI/air-gapped. v0.6 ships basic support (refuse if a dep isn't cached);
   v0.7+ may add vendoring. Confirm.

---

## Milestone 74 — Polish & v0.6.0 release

- [ ] **Edit: `crates/red-cli/src/main.rs`** `HELP` — add a `PACKAGES`
      section documenting `red pkg init`/`add`/`install`/`build`/`run`,
      `load-plugin`, `--module-path`, and the `--allow-shell` gating of
      `load-plugin`.
- [ ] **Edit: `crates/red-cli/src/main.rs`** — `--help` lists
      `load-plugin` under `--allow-shell`-gated natives.
- [ ] **New: `examples/packages/../../README.md`** — walkthrough of the
      `mathutils` example (path-dep version, hermetic). Git-dep version
      documented but not tested (network).
- [ ] **Edit: `crates/red-eval/src/plugin.rs`** module docs — document the
      three drift layers, the patch-ok/minor-exact policy, the layout hash,
      the allocator discipline (system allocator, opaque handles), and the
      threat model (cdylib = full process privileges, gated on
      `--allow-shell`).
- [ ] **Edit: `crates/red-pkg/src/lib.rs`** module docs — document the
      `package.red` schema, source protocols (git/path), `red.lock`
      format, conflict policy (refuse), and the `~/.red/cache/` layout.
- [ ] **Audit `EvalError` rendering** for new error sources:
      - `PluginAbiMismatch` / `PluginLayoutMismatch` / `PluginMissingSymbol` /
        `PluginLoadFailed` (M70).
      - `DependencyCycle` / `DependencyConflict` / `RedVersionConflict` /
        `ManifestParse` / `GitFetchFailed` / `BuildFailed` (M71/M73).
  Each renders `*** Error: [loc: ]<category>: <message>` with the plugin
  name / package name / chain as appropriate.
- [ ] **Golden fixtures** (one per error case):
      - `programs_errors/plugin_abi_mismatch.red` (loads a stub cdylib
        with a bad version; asserts the error substring).
      - `programs_errors/plugin_missing_symbol.red`.
      - `programs_errors/load_plugin_shell_disabled.red` (no
        `--allow-shell`).
      - `programs_errors/dependency_cycle.red` (constructs a manifest
        with a cycle — may need a test-only `pkg` native that takes a
        manifest block directly, since `red pkg` is a CLI subcommand).
- [ ] **Property test:** `mold(parse(mold(v)))` round-trip for the
      `package.red` block form (extend `red-core/tests/property.rs` or add
      a `red-pkg/tests/property.rs`).
- [ ] **Run `cargo bench --bench eval`**; record in `../../BENCHMARKS.md` under
      "v0.6.0". The plugin trampoline adds a per-native-call FFI cost —
      expected to be neutral on existing benches (no plugin natives in
      fib/ackermann/sum_loop). If any bench regresses >5%, investigate the
      trampoline's arg-marshalling cost.
- [ ] **Run `cargo clippy --workspace --all-targets -- -D warnings`**; fix.
- [ ] **Run `cargo fmt --all --check`**; fix.
- [ ] **Update `../../project-brief.md`:**
  - [ ] Remove "modules / `import` / `export`" and "closures" from the
        v0.5-candidates note (already done by plan6's M65; verify).
  - [ ] Add "Package Manager & Native Plugins (v0.6)" subsection under
        "Binding & contexts": the cdylib plugin layer, the
        `red-plugin-abi` shim, the three drift layers, `load-plugin`
        gating, `package.red` manifest, `red pkg` CLI, `red.lock`.
  - [ ] Add `PluginHandle` to the value-model section (or note it's in
        `env.rs`, not a `Value` variant).
  - [ ] Note `red pkg run` and `--module-path` interactions.
  - [ ] Update "Deferred" list: remove `routine!` FFI (superseded by the
        plugin layer — see "Relationship to `routine!`"); add "WASM
        plugins", "plugin sandboxing", "plugin hot-reload", "central
        registry server", "multiple versions side-by-side".
- [ ] **Update `../../architecture.md`:**
  - [ ] New section "Plugin system" — cdylib + `dlopen`, opaque handles,
        the `red-plugin-abi` crate, the trampoline, the three drift
        layers, allocator discipline, threat model.
  - [ ] New section "Package manager" — `package.red` schema, source
        resolution, `red.lock`, `~/.red/cache/` layout, conflict policy.
  - [ ] Note the `Env::plugins`/`plugin_handles` fields and the
        `invalidate_native_index` call after `load_plugin`.
  - [ ] Note the `RunOptions::plugin_paths`/`auto_load_plugins` fields.
- [ ] **Update `../../README.md`:**
  - [ ] Bump version to v0.6.0.
  - [ ] Add "Packages & Native Plugins" bullet under "What's implemented".
  - [ ] Add `load-plugin`/`import` (plugin) to the natives list.
  - [ ] Add `red pkg init`/`add`/`install`/`build`/`run` to the CLI
        section.
  - [ ] Update "Known gaps" with the new deferrals (WASM plugins, plugin
        sandboxing, plugin hot-reload, central registry, multiple
        versions side-by-side).
  - [ ] Note that `routine!` FFI is superseded by the plugin layer (or
        reframe it as "inline `ccall` binding generator, deferred").
- [ ] **Final `cargo test --workspace`** green.
- [ ] **Final `cargo test --workspace --features force-walk`** green.
- [ ] **Final `cargo clippy --workspace --all-targets -- -D warnings`** clean.
- [ ] **Final `cargo fmt --all --check`** clean.
- [ ] **Tag release `v0.6.0`.**

---

## Open questions

1. **Multiple versions side-by-side (M73).** v0.6 refuses conflicts. v0.7+
   may allow via namespaced module paths (`strings@1.0/foo` vs
   `strings@2.0/foo`), requiring the M61 module cache to key by
   `(name, version)` rather than `name`. Recommendation: defer to v0.7.
   Confirm before implementing M73.
2. **`cargo` invocation (M73).** `std::process::Command::new("cargo")` —
   what if `cargo` isn't on PATH? Recommendation: clear error
   `*** pkg error: cargo not found on PATH (required to build native deps)`.
   Confirm.
3. **Layout hash derivation (M72).** `abi_stable` (robust, adds dep) vs
   hand-rolled (zero deps, manual maintenance) vs no Layer 2 (rely on
   Layer 1 minor-exact). Recommendation: `abi_stable` — the layout hash is
   defense-in-depth and worth the dep. Confirm before implementing M72.
4. **Default git URL convention (M71).** `name "1.0"` shorthand — fetch
   from where? Options: (a) a real `red-pkg` GitHub org (requires setting
   one up); (b) no default, must specify `git:`. Recommendation: (b) for
   v0.6 (no registry, no default URL); revisit when a registry exists.
   Confirm.
5. **Cache location (M71).** `~/.red/cache/` (user-global) vs
   `./red_cache/` (project-local)? Recommendation: `~/.red/cache/` with
   `--cache-dir` override. Confirm.
6. **`red.lock` format (M71).** TOML (chosen) vs. a Red block
   (`red.lock.red`)? Recommendation: TOML — `red.lock` is tooling-authored,
   not user-authored; `serde` out of the box. Confirm.
7. **`init` template scope (M71).** Just `package.red` + `lib/` dirs, or
   also a `main.red` hello-world? Recommendation: include `lib/main.red`
   with `Red [] print "hello"`. Confirm.
8. **Plugin unload (deferred).** Unloading a cdylib with live `Rc`s is UB.
   v0.6 holds plugins for process lifetime. v0.7+ could support unload if
   every value the plugin touched is first released — a reference-count
   audit. Recommendation: defer to v0.7+ if a real need (memory pressure,
   hot-reload) surfaces. Confirm defer.
9. **Sandboxing (deferred).** cdylib plugins run with full process
   privileges (same as `call`/`shell`). Sandboxing would mean WASM
   (declined) or seccomp/AppArmor (platform-specific, fragile).
   Recommendation: defer; document the threat model clearly. Confirm defer.
10. **`routine!` FFI (deferred / superseded).** The cdylib plugin layer
    supersedes the "ship a binding generator" reading of `routine!`. A
    future `routine!` could be *itself a plugin* (a cdylib that registers
    `routine` as a native, generating `ccall` bindings inline). Or
    `routine!` could remain a separate inline-FFI feature. Recommendation:
    reframe `routine!` as "inline `ccall` binding generator, deferred" in
    docs; the plugin layer is the new extension mechanism. Confirm.
