# Semantic Types (M170–M178)

Semantic types are parse-backed schemas over a base datatype — a value
*stays* its base type at runtime, but a registered type adds a validation
rule, a generated predicate, a generated constructor, and (since M175) a
tagged-value construction path.

## Defining a type

```red
define-type 'rgb! 'tuple! [r: byte g: byte b: byte]        ; positional
define-type 'port! 'integer! [range 1 65535]              ; scalar
define-type 'slug! 'string! [some slug-char]              ; streamed
define-type 'even-count! 'block! [3 segment]              ; count form
```

The schema shape is derived from the base datatype:

| Shape | Bases | Schema form |
|---|---|---|
| Positional | `tuple! pair! date! duration!` | `field: constraint …` |
| Scalar | `integer! float! decimal! percent! money!` | `[range lo hi]` / `[where [pred]]` / `[primitive]` |
| Streamed | `string! binary! block! paren! url! file! issue! email! tag!` | a parse-dialect rule |
| Named | `object! module! map! hash!` | `field: constraint …` |

Positional/scalar constraints:

- `byte`, `integer`, `positive-integer`, `non-negative-integer`,
  `nonzero-integer`, `number`
- `range lo hi` — bare word form or wrapped (`[range 1 65535]`)
- `where [predicate-block]`
- `optional` before a constraint marks a field optional
- Count forms (streamed, M178): `3 segment` (exactly 3) or
  `2 to 5 alpha` (between 2 and 5)

**Dependent constraints** (M178): a `range` operand may be a paren,
evaluated at check time with the already-captured field words in scope:

```red
define-type 'iso-date! 'tuple! [
    year: integer
    month: range 1 12
    day: range 1 (days-in-month year month)   ; leap-year aware
]
valid? 'iso-date! 24.2.29   ; true  (2024 leap year, 2-digit since tuple bytes are 0..255)
valid? 'iso-date! 23.2.29   ; false (2023)
```

## Checking values

```red
rgb? 1.2.3                  ; generated predicate
valid? 'rgb! 1.2.3          ; direct check
validate 'rgb! 300.2.3      ; rich error on failure
```

NOTE: when a semantic type shares its name with a builtin type predicate
(e.g. `port!`), the builtin wins and the semantic predicate is not
registered — use `valid?`/`validate` for those.

## Constructors (untagged) vs `make` (tagged)

The generated constructor takes the components and returns the **plain**
base value:

```red
rgb 255 0 0                ; → 255.0.0 (tuple!, untagged)
```

`make <type>! <value>` validates and returns a **tagged** value
(M175) — the base value carrying the semantic type's name as metadata:

```red
t: make rgb! 1.2.3
type? t                    ; tuple!          (tags are transparent)
mold t                     ; 1.2.3
t = 1.2.3                  ; true            (compares as the inner value)
t * 2                      ; 2.4.6           (arithmetic on the inner value)
rgb? t                     ; true            (predicates accept it)
semantic-type? t           ; 'rgb!           (the discriminator)
mold/tagged t              ; "make rgb! 1.2.3" (the reconstructing form)
```

`semantic-type?` returns the value's tag as a `lit-word!`, a
`semantic-type!` value's name as a `lit-word!`, or `none` for untagged
values. `copy` preserves the tag.

Invalid values raise the M177 rich error:

```red
make port! 70000
; *** Error: script error: Invalid port: must be in range 1..65535, got 70000
```

## Func-spec annotations

A semantic type in a func-spec annotation validates at call time
(M176/M177):

```red
f: func [p [port!]] [p]
f 443                     ; ok
f 99999                   ; type error: arg 1 expected port! (base integer!), got integer!
```

## Reference

- Implementation: `crates/red-eval/src/semantic.rs` (schema compiler,
  predicates, constructors, `valid?`/`validate`), `crates/red-core/src/
  value.rs` (`SemanticTypeDef`, `Value::SemanticType`, `Value::
  SemanticTagged`, `unwrap_semantic`).
- Plan: `docs/plans/plan18-semantic-types.md`; design notes in
  `docs/plans/future-plan-parse_backed_semantic_types.md`.
- Fixture: `crates/red-eval/tests/programs/semantic_tagged_values.red`
  (runs in the golden + VM/walker parity suites).
