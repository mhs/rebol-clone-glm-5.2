# Parse-Backed Semantic Types for a Modern Rebol/Red Clone

## Overview

This document describes an approach for adding **semantic subtypes** to a Rebol/Red-style language while preserving the language's core strengths: compact literal syntax, dialects, dynamic values, and `parse` as a general-purpose grammar engine.

The central idea is simple:

> Keep primitive datatypes broad and flexible, but allow semantic types to be defined as schemas backed by parse rules. Any base datatype that can expose a **component view** of its structure can host semantic subtypes.

A raw value says what the representation is. A semantic type says what the value *means*.

| Literal | Raw Type | Possible Semantic Type |
|---|---|---|
| `255.0.0` | `tuple!` | `rgb!` (color) |
| `192.168.1.10` | `tuple!` | `ipv4!` (network address) |
| `100x50` | `pair!` | `size2d!` (dimensions) |
| `"user-42"` | `string!` | `slug!` (URL-safe identifier) |
| `8080` | `integer!` | `port!` (TCP/UDP port) |
| `[a b c]` | `block!` | `path!` (segmented lookup key) |
| `make object! [name: "Ada"]` | `object!` | `person!` (record with required fields) |
| `http://example.com` | `url!` | `http-url!` (scheme-restricted URL) |
| `3-7-2026` | `date!` | `iso-date!` (calendar date with constraints) |

Each of these is the same value at runtime as its base datatype. But in different contexts they mean different things:

```rebol
paint 255.0.0          ; RGB color
connect 192.168.1.10   ; IPv4 address
open-port 8080         ; TCP port
navigate "user-42"     ; slug
```

A parse-backed semantic type system lets us define and validate those meanings explicitly:

```rebol
type rgb!: tuple! [r: byte  g: byte  b: byte]
type ipv4!: tuple! [a: byte  b: byte  c: byte  d: byte]
type port!: integer! [range 1 65535]
type slug!: string! [some slug-char]
type path!: block! [some segment]
```

The values remain ordinary base values at runtime, but functions, dialects, and APIs can require more specific semantic shapes.

---

## Motivation

Rebol and Red already provide compact domain-oriented literals. The challenge is that a raw datatype often does not fully express the intended meaning.

For instance, `255.0.0` could be:

- an RGB color
- a semantic version
- a protocol version
- a compact numeric identifier

Likewise, `"user-42"` could be:

- a URL slug
- a username
- a freeform tag
- a CSV field

And `8080` could be:

- a TCP port
- an HTTP status code
- a memory offset
- a generic integer

A raw type check can answer this:

```rebol
tuple? 255.0.0      ; true
string? "user-42"   ; true
integer? 8080       ; true
```

But it cannot answer these directly:

```rebol
rgb? 255.0.0        ; true
slug? "user-42"     ; true
port? 8080          ; true

ipv4? 255.0.0       ; false
slug? "Ada Lovelace"; false (contains space)
port? 99999         ; false (out of range)
```

Parse-backed semantic types provide that second layer, uniformly across every base datatype that can expose a component view.

---

## Design Goals

The design should:

1. Preserve Rebol's compact literal syntax.
2. Avoid turning every domain concept into a heavyweight object.
3. Allow semantic constraints over **any** base datatype's component view — positional (tuple/pair), streamed (string/block), named (object), or scalar (integer/number).
4. Work naturally inside dialects.
5. Use `parse` or a parse-compatible schema dialect as the underlying validation engine.
6. Provide readable error messages.
7. Allow both dynamic validation and optional static/tooling analysis.
8. Remain simple enough to fit Rebol's philosophy.

The goal is not to replace any base datatype with a rigid type hierarchy. The goal is to let specific contexts say:

> I accept a value of this base type, but only one that conforms to this semantic schema.

---

## Core Concept

A semantic type has three parts:

```rebol
type <name>!: <base>! [
    <schema>
]
```

| Part | Meaning |
|---|---|
| `<name>!` | Name of the semantic type |
| `<base>!` | Underlying base datatype |
| schema block | Shape and constraints over the value's component view |

The raw value is still its base type:

```rebol
red: 255.0.0
type? red
; tuple!

slug: "user-42"
type? slug
; string!

p: 8080
type? p
; integer!
```

But semantic predicates can be generated:

```rebol
rgb? red            ; true
slug? slug          ; true
port? p             ; true

ipv4? red           ; false
port? "8080"        ; false (string, not integer)
```

Function specs can then use semantic types:

```rebol
paint: func [color [rgb!]] [...]
connect: func [address [ipv4!] port [port!]] [...]
navigate: func [s [slug!]] [...]
```

Now this is valid:

```rebol
paint 255.0.0
connect 192.168.1.10 443
navigate "user-42"
```

but these are rejected:

```rebol
paint 192.168.1.10
; error: expected rgb!, got tuple! with 4 components

connect 999.1.2.3 443
; error: component a must be byte, got 999

navigate "Ada Lovelace"
; error: expected slug!, got string! with disallowed character at index 3
```

---

## Why Use `parse`?

Rebol's `parse` dialect is already a grammar system for recognizing structure in values. It can parse strings, blocks, and in a modern clone can be generalized or layered to parse the component representation of any value.

The key abstraction is the **component view**: every base datatype provides a way to turn a value into something `parse` can consume. For positional types that is a block of components; for strings it is a character stream; for blocks it is the block itself; for objects it is a field/value map; for scalars it is a single-element block.

```rebol
to-components 255.0.0
; [255 0 0]

to-components "user-42"
; "user-42"   ; parsed as a char stream

to-components 8080
; [8080]

to-components [a b c]
; [a b c]     ; parsed as a token stream

to-components make object! [name: "Ada" age: 36]
; [name "Ada" age 36]   ; parsed as field/value pairs
```

Once exposed, standard parse-style rules can validate it:

```rebol
byte-rule: [
    set n integer! (
        unless all [n >= 0 n <= 255] [fail]
    )
]

rgb-rule:   [byte-rule byte-rule byte-rule end]
ipv4-rule:  [byte-rule byte-rule byte-rule byte-rule end]
port-rule:  [set n integer! (unless all [n >= 1 n <= 65535] [fail]) end]
slug-rule:  [some slug-char-rule end]
path-rule:  [some segment-rule end]
```

Then predicates can be implemented conceptually as:

```rebol
rgb?: func [value] [
    all [tuple? value  parse to-components value rgb-rule]
]

port?: func [value] [
    all [integer? value  parse to-components value port-rule]
]

slug?: func [value] [
    all [string? value  parse to-components value slug-rule]
]
```

This makes `parse` the validation engine underneath semantic types, **uniformly across all base types**.

---

## The Component-Extraction Protocol

Each supported base datatype registers an extraction strategy. This is the bridge between raw values and the parse engine.

| Base Type | Component Representation | Schema Shape |
|---|---|---|
| `tuple!` | block of tuple components | positional |
| `pair!` | block of `[x y]` | positional |
| `integer!` / `number!` | single-element block `[n]` | scalar |
| `string!` | character stream | streamed |
| `binary!` | byte stream | streamed |
| `block!` | original block (token stream) | streamed |
| `object!` | field/value pairs | named |
| `url!` | string of the URL | streamed |
| `date!` | block of `[year month day]` | positional |
| `time!` | block of `[hours minutes seconds]` | positional |

This gives the parse engine a consistent model:

```text
base value → component view → parse rule
```

A base type is "semantic-type-ready" if it supplies a `to-components` rule. Adding support for a new base type is therefore a matter of registering a new extractor, not changing the schema compiler.

### Positional vs Streamed vs Named vs Scalar

Schemas take one of four shapes depending on the extractor:

- **Positional** (tuple, pair, date, time): a fixed-arity sequence of named fields.
  ```rebol
  type rgb!: tuple! [r: byte  g: byte  b: byte]
  ```
- **Scalar** (integer, number): a single value with a constraint.
  ```rebol
  type port!: integer! [range 1 65535]
  type percent!: number! [range 0 100]
  ```
- **Streamed** (string, binary, block, url): a sequence of tokens or characters, possibly open-ended.
  ```rebol
  type slug!: string! [some slug-char]
  type path!: block! [some segment]
  ```
- **Named** (object): a set of required/optional fields with per-field constraints.
  ```rebol
  type person!: object! [
      name: string
      age:  optional [range 0 150]
      email: optional email!
  ]
  ```

The schema dialect uses the same surface syntax in all four cases; the extractor determines how the schema is compiled.

---

## Semantic Types by Base Datatype

The following subsections illustrate each family. Together they show that the system is generic, not specific to tuples and pairs.

### `tuple!`-backed examples

```rebol
type rgb!: tuple! [
    r: byte
    g: byte
    b: byte
]

type rgba!: tuple! [
    r: byte
    g: byte
    b: byte
    a: byte
]

type ipv4!: tuple! [
    a: byte
    b: byte
    c: byte
    d: byte
]

type semver!: tuple! [
    major: non-negative-integer
    minor: non-negative-integer
    patch: non-negative-integer
]
```

Usage:

```rebol
red: 255.0.0
transparent-red: 255.0.0.128
server: 192.168.1.10
version: 1.4.2

rgb? red               ; true
rgba? red              ; false
rgba? transparent-red  ; true
ipv4? server           ; true
rgb? server            ; false
semver? version        ; true
```

Note that `1.4.2` is also structurally a valid `rgb!` (three byte-sized values). Semantic type validity does not always imply semantic intent; intent is supplied by the context (function spec or dialect keyword) that requires a specific semantic type.

### `pair!`-backed examples

```rebol
type size2d!: pair! [
    width:  positive-integer
    height: positive-integer
]

type point2d!: pair! [
    x: integer
    y: integer
]

type offset2d!: pair! [
    dx: integer
    dy: integer
]
```

Usage:

```rebol
button-size: 100x50
position: 20x30
movement: -5x10

size2d?  button-size   ; true
point2d? position      ; true
offset2d? movement     ; true
size2d?  movement      ; false (negative)
```

### `integer!` / `number!`-backed examples

Scalar schemas constrain a single value.

```rebol
type port!: integer! [
    range 1 65535
]

type percent!: number! [
    range 0 100
]

type nonzero!: integer! [
    where [value <> 0]
]

type unix-timestamp!: integer! [
    range 0 4102444800   ; year 2100
]
```

Usage:

```rebol
port? 443              ; true
port? 99999            ; false
port? "443"            ; false (string, not integer)

percent? 50            ; true
percent? 50.5          ; true
percent? 150           ; false
```

These are single-component schemas. They compile to a one-element positional rule.

### `string!`-backed examples

Streamed schemas parse over the character stream of the string.

```rebol
type slug!: string! [
    some slug-char
]

type email!: string! [
    some alpha-or-digit
    "@"
    some slug-char
    "."
    some alpha
]

type uuid!: string! [
    8 hex-char "-" 4 hex-char "-" 4 hex-char
    "-" 4 hex-char "-" 12 hex-char
]

type hex-color!: string! [
    "#" some hex-char
]
```

Usage:

```rebol
slug? "user-42"              ; true
slug? "Ada Lovelace"         ; false (space)

email? "ada@example.com"     ; true
email? "not-an-email"        ; false

uuid? "550e8400-e29b-41d4-a716-446655440000"  ; true
```

### `block!`-backed examples

Streamed schemas parse over the tokens of the block itself.

```rebol
type path!: block! [
    some segment
]

type csv-row!: block! [
    field some ["," field]
]

type config!: block! [
    some [set name word! set value [string! | integer! | block!]]
]
```

Usage:

```rebol
path? [a b c]                        ; true
path? [1 2 3]                        ; depends on segment rule
csv-row? ["a" "b" "c"]               ; true

config? [host "localhost" port 8080] ; true
```

### `object!`-backed examples

Named schemas validate the field/value map of an object.

```rebol
type person!: object! [
    name:  string
    age:   optional [range 0 150]
    email: optional email!
]

type matrix!: object! [
    rows:    positive-integer
    cols:    positive-integer
    data:    block
    where    [length? data = rows * cols]
]

type rect!: object! [
    origin: point2d!
    size:   size2d!
]
```

Usage:

```rebol
ada: make object! [name: "Ada" age: 36]
person? ada     ; true

bad: make object! [name: 123]
person? bad     ; false (name must be string)
```

Object-backed semantic types overlap conceptually with regular object prototypes. The distinction is that a semantic type is a *validator over* an object's fields rather than a constructor; the object itself remains an ordinary `object!` value.

### `url!`-backed examples

```rebol
type http-url!: url! [
    "http" "s" "://" some url-char
]

type ws-url!: url! [
    "ws" "s" "://" some url-char
]
```

Usage:

```rebol
http-url? http://example.com      ; true
http-url? ftp://example.com       ; false
```

### `date!` / `time!`-backed examples

```rebol
type iso-date!: date! [
    year:  integer
    month: range 1 12
    day:   range 1 days-in-month year month
]

type business-time!: time! [
    hours:   range 9 17
    minutes: range 0 59
    seconds: range 0 59
]
```

Dependent constraints (such as `days-in-month year month`) are an advanced feature and need not ship in the first version, but the component view makes them expressible.

---

## Public Syntax vs Internal Parse Rules

Although raw parse rules are powerful, they may not be ideal as the public type-definition syntax. A nicer public schema dialect is used at the surface:

```rebol
type rgb!: tuple! [
    r: byte
    g: byte
    b: byte
]
```

Internally, that compiles to a parse rule like:

```rebol
[
    set r byte-rule
    set g byte-rule
    set b byte-rule
    end
]
```

A scalar schema compiles to a single-element rule:

```rebol
type port!: integer! [range 1 65535]
; compiles to:
[
    set n integer! (
        unless all [n >= 1 n <= 65535] [fail]
    )
    end
]
```

A streamed schema compiles to a stream rule:

```rebol
type slug!: string! [some slug-char]
; compiles to:
[
    some slug-char-rule
    end
]
```

A named schema compiles to an alternating field/value rule:

```rebol
type person!: object! [name: string  age: optional [range 0 150]]
; compiles to:
[
    'name set name string!
    opt ['age set age [integer! (unless all [age >= 0 age <= 150] [fail])]]
    end
]
```

This keeps the user-facing syntax clean while still using `parse` internally.

```text
schema dialect → parse rule → runtime validator
```

This gives the language a Rebol-native type mechanism without requiring a completely separate type-checking language, and it works identically across positional, scalar, streamed, and named schemas.

---

## Function Specs with Semantic Types

A key use case is function argument validation.

```rebol
paint:   func [color [rgb!]]                          [...]
connect: func [address [ipv4!] port [port!]]          [...]
resize:  func [target [object!] size [size2d!]]       [...]
navigate:func [s [slug!]]                             [...]
open:    func [p [port!]]                             [...]
render:  func [m [matrix!]]                           [...]
fetch:   func [u [http-url!]]                         [...]
```

These specs are more expressive than raw base types:

```rebol
paint:    func [color [tuple!]]    [...]   ; says only "representation"
connect:  func [address [tuple!]]  [...]
resize:   func [size [pair!]]      [...]
navigate: func [s [string!]]       [...]
open:     func [p [integer!]]      [...]
```

The raw specs say only what the representation is. The semantic specs say what the value means.

The function machinery checks:

1. Is `<name>!` a built-in datatype?
2. If not, is it a registered semantic type?
3. If yes, validate the value against the semantic type's base and parse rule.

This allows semantic types to coexist with built-in types.

---

## Dialect Integration

Semantic types become especially powerful inside dialects.

Consider a UI dialect:

```rebol
view [
    window "Demo" size 640x480 [
        at 20x20
        button "Save" size 100x32
        box 200x100 color 255.0.0
        link "Docs" href http://example.com
    ]
]
```

The dialect grammar can declare expectations spanning multiple base types:

```rebol
ui-schema: [
    window string! 'size size2d! block!
    at point2d!
    button string! 'size size2d!
    box size2d! 'color rgb!
    link string! 'href http-url!
]
```

Or internally, a parser can use semantic validators:

```rebol
parse-ui: func [tokens] [
    parse tokens [
        some [
            'at   set p pair!   (validate point2d! p)
          | 'size set s pair!   (validate size2d!  s)
          | 'color set c tuple! (validate rgb!     c)
          | 'href set u url!    (validate http-url! u)
          | skip
        ]
    ]
]
```

This keeps dialects readable while adding meaningful validation, and it gives dialect authors a shared validation framework instead of forcing every dialect to hand-roll checks.

---

## Type Definition Model

A semantic type definition can be represented as metadata:

```rebol
make semantic-type! [
    name: 'port!
    base: integer!
    shape: 'scalar
    schema: [range 1 65535]
    parse-rule: [...compiled...]
]
```

The `shape` field records whether the schema is positional, scalar, streamed, or named; it is derived from the base type's registered extractor.

The runtime registry might contain:

```rebol
semantic-types: make map! [
    rgb!       make semantic-type! [...]
    ipv4!      make semantic-type! [...]
    port!      make semantic-type! [...]
    slug!      make semantic-type! [...]
    path!      make semantic-type! [...]
    person!    make semantic-type! [...]
    http-url!  make semantic-type! [...]
]
```

A generic validator can then work like this:

```rebol
valid?: func [type value] [
    spec: select semantic-types type
    all [
        spec
        spec/base = type? value
        parse to-components value spec/parse-rule
    ]
]
```

Where `to-components` dispatches on the base type's registered extractor:

```rebol
to-components: func [value] [
    case [
        tuple?  value [to-block value]
        pair?   value [reduce [value/x value/y]]
        integer? value [reduce [value]]
        number? value [reduce [value]]
        string? value [value]
        binary? value [value]
        block?  value [value]
        url?    value [form value]
        object? value [mold-object-pairs value]
        date?   value [reduce [value/year value/month value/day]]
        time?   value [reduce [value/hour value/minute value/second]]
        true [reduce [value]]
    ]
]
```

Adding a new base type means adding a new case here and registering it; the schema compiler and validator are unchanged.

---

## Primitive Constraint Types

Semantic schemas need a small vocabulary of primitive constraints. These are independent of base type — they apply to whatever component the schema is currently validating.

```rebol
byte                ; integer from 0 to 255
integer             ; any integer
non-negative-integer
positive-integer
nonzero-integer
number              ; integer or decimal
percent             ; number from 0 to 100
alpha               ; alphabetic character
digit
hex-char
slug-char           ; letter, digit, or hyphen
url-char
segment             ; a valid path segment (word or string)
```

These can themselves be parse rules or semantic predicates.

For example:

```rebol
type byte!: integer! [
    value >= 0
    value <= 255
]
```

or, internally:

```rebol
byte-rule: [
    set n integer! (
        unless all [n >= 0 n <= 255] [fail]
    )
]
```

A scalar semantic type is simply a single-component schema whose constraint is one of these primitives or a `range`/`where` clause:

```rebol
type port!: integer! [range 1 65535]
```

The schema dialect can hide the distinction between positional, scalar, streamed, and named compilation.

---

## Error Reporting

One benefit of using a schema dialect instead of raw parse rules is better error reporting.

Given:

```rebol
type ipv4!: tuple! [
    a: byte
    b: byte
    c: byte
    d: byte
]
```

the validator can report:

```text
Invalid ipv4!: expected 4 components, got 3
```

or:

```text
Invalid ipv4!: component c must be byte, got 300
```

For streamed schemas:

```text
Invalid slug!: disallowed character ' ' at index 3
```

For named schemas:

```text
Invalid person!: field age must be in range 0..150, got 200
Invalid person!: missing required field name
```

For scalar schemas:

```text
Invalid port!: expected integer in range 1..65535, got 99999
Invalid port!: expected integer!, got string!
```

The named fields in positional and named schemas are valuable even when the runtime representation is positional or field-map-based. They allow the language to explain failures in domain terms.

---

## Constructors

Semantic types can generate constructors.

```rebol
rgb:    func [r [byte!] g [byte!] b [byte!]]                [make rgb!      reduce [r g b]]
ipv4:   func [a [byte!] b [byte!] c [byte!] d [byte!]]      [make ipv4!     reduce [a b c d]]
port:   func [p [port!]]                                    [make port!     p]
slug:   func [s [slug!]]                                    [make slug!     s]
person: func [name [string!] . age [optional integer!]]     [make person!   reduce [name age]]
```

Usage:

```rebol
red:    rgb 255 0 0
server: ipv4 192 168 1 10
p:      port 443
s:      slug "user-42"
ada:    person "Ada" 36
```

The return values could still be raw base values:

```rebol
type? red
; tuple!

type? p
; integer!
```

Or the implementation could optionally tag them with semantic metadata:

```rebol
semantic-type? red
; rgb!

semantic-type? p
; port!
```

This is an important design choice (see below).

---

## Tagged vs Untagged Semantic Values

There are two possible runtime models.

### Untagged Model

In the untagged model, semantic types are validation rules only.

```rebol
red: 255.0.0
type? red
; tuple!
rgb? red
; true

p: 443
type? p
; integer!
port? p
; true
```

Advantages:

- Simple runtime model.
- Values remain ordinary Rebol values.
- No hidden metadata.
- Easy interop with existing code.
- Fits dynamic dialect interpretation.

Disadvantages:

- Intent is contextual, not carried by the value.
- `1.2.3` can validate as both `rgb!` and `semver!`.
- Harder to preserve semantic meaning across APIs without explicit specs.

### Tagged Model

In the tagged model, a value can carry an explicit semantic type tag.

```rebol
red: make rgb! 255.0.0
type? red
; tuple!
semantic-type? red
; rgb!

p: make port! 443
type? p
; integer!
semantic-type? p
; port!
```

Advantages:

- Intent travels with the value.
- Better tooling and debugging.
- Less ambiguity between structurally overlapping types.

Disadvantages:

- More complex runtime.
- Equality and serialization semantics become harder.
- May feel less Rebol-like if overused.

### Recommended Default

Use the untagged model by default, with optional tagging where needed.

```text
Default: semantic types are validators over values.
Optional: constructors can attach semantic tags for stronger intent preservation.
```

This keeps the system lightweight while leaving room for stronger semantics.

---

## Validation Lifecycle

Semantic validation can happen at several points.

### 1. Function Call Time

```rebol
paint: func [color [rgb!]] [...]
paint 255.0.0
```

The function dispatcher validates `255.0.0` against `rgb!` before entering the body.

### 2. Dialect Evaluation Time

```rebol
view [
    box 100x50 color 255.0.0
    link "Docs" href http://example.com
]
```

The dialect interpreter validates that `100x50` is a `size2d!`, `255.0.0` is an `rgb!`, and the URL is an `http-url!`.

### 3. Constructor Time

```rebol
red: rgb 255 0 0
p:   port 443
```

The constructor validates components before producing the value.

### 4. Assignment Time

A stricter language could allow typed bindings:

```rebol
red [rgb!]: 255.0.0
p   [port!]: 443
```

or:

```rebol
red: rgb! 255.0.0
p:   port! 443
```

This is more static-feeling and should probably be optional.

---

## Parse Rule Compilation

A positional schema block:

```rebol
[r: byte  g: byte  b: byte]
```

compiles to:

```rebol
[set r byte-rule  set g byte-rule  set b byte-rule  end]
```

A scalar schema:

```rebol
[range 1 65535]
```

compiles to:

```rebol
[set n integer! (unless all [n >= 1 n <= 65535] [fail]) end]
```

A streamed schema:

```rebol
[some slug-char]
```

compiles to:

```rebol
[some slug-char-rule  end]
```

A named schema:

```rebol
[name: string  age: optional [range 0 150]]
```

compiles to:

```rebol
[
    'name set name string!
    opt ['age set age [integer! (unless all [age >= 0 age <= 150] [fail])]]
    end
]
```

A schema with optional positional elements:

```rebol
type version!: tuple! [
    major: integer
    minor: integer
    patch: optional integer
]
```

compiles to:

```rebol
[set major integer!  set minor integer!  opt [set patch integer!]  end]
```

A schema with repetition:

```rebol
type path!: block! [
    some segment
]
```

compiles to:

```rebol
[some segment-rule  end]
```

The schema dialect does not need to expose every feature of `parse` immediately. It can start small and grow, but it should support all four shapes (positional, scalar, streamed, named) from the start so the system is genuinely generic.

---

## Schema Dialect Features

A useful first version could support:

```rebol
name: constraint
optional constraint
some constraint
any constraint
one-of [constraint-a constraint-b]
range 0 255
where [predicate]
```

Examples:

```rebol
type rgb!: tuple! [
    r: byte
    g: byte
    b: byte
]

type semver!: tuple! [
    major: non-negative-integer
    minor: non-negative-integer
    patch: non-negative-integer
]

type port!: integer! [
    range 1 65535
]

type percent!: number! [
    range 0 100
]

type slug!: string! [
    some slug-char
]

type path!: block! [
    some segment
]

type person!: object! [
    name:  string
    age:   optional [range 0 150]
    email: optional email!
]
```

A later version could support dependent validation:

```rebol
type iso-date!: date! [
    year:  integer
    month: range 1 12
    day:   range 1 days-in-month year month
]
```

But that should not be required for the first design.

---

## Relationship to Existing Rebol/Red Type Specs

Traditional function specs might look like this:

```rebol
connect: func [address [tuple!] port [integer!]] [...]
navigate: func [s [string!]] [...]
```

The proposed model extends this naturally:

```rebol
connect: func [address [ipv4!] port [port!]] [...]
navigate: func [s [slug!]] [...]
```

The function machinery checks:

1. Is `ipv4!` a built-in datatype?
2. If not, is it a registered semantic type?
3. If yes, validate the value against the semantic type's base and parse rule.

This allows semantic types to coexist with built-in types.

---

## Relationship to Dialects

In Rebol-like languages, dialects often rely on value types to guide interpretation.

For example:

```rebol
layout [
    text "Hello"
    size 200x40
    color 255.0.0
    href http://example.com
]
```

Without semantic types, the dialect sees:

```text
string!
pair!
tuple!
url!
```

With semantic types, the dialect can say:

```text
size  expects size2d!
color expects rgb!
href  expects http-url!
```

This gives dialect authors a shared validation framework instead of forcing every dialect to hand-roll checks.

---

## Serialization

Because semantic values are often represented as ordinary base values, serialization can stay simple.

```rebol
red: 255.0.0
server: 192.168.1.10
p: 443
s: "user-42"
```

Serialized as data:

```rebol
[
    color 255.0.0
    host  192.168.1.10
    port  443
    slug  "user-42"
]
```

The receiving context determines semantics.

For tagged semantic values, serialization may need optional annotations:

```rebol
make rgb! 255.0.0
make ipv4! 192.168.1.10
make port! 443
make slug! "user-42"
```

or:

```rebol
#rgb 255.0.0
#ipv4 192.168.1.10
#port 443
#slug "user-42"
```

The untagged model should remain the default for compatibility and readability.

---

## Tooling Opportunities

Parse-backed semantic types could support tooling without requiring full static typing.

Possible tooling features:

- Validate configuration files.
- Lint dialect blocks.
- Generate documentation from type schemas.
- Generate constructors and predicates.
- Generate test cases.
- Provide editor hints for expected semantic values.
- Warn about ambiguous value usage (e.g. a tuple that satisfies both `rgb!` and `semver!`).

Example generated docs:

```text
port!
Base: integer!
Shape: scalar
Constraint: range 1..65535

rgb!
Base: tuple!
Shape: positional
Components:
  r byte 0..255
  g byte 0..255
  b byte 0..255

slug!
Base: string!
Shape: streamed
Constraint: some slug-char
```

Example generated predicate:

```rebol
port?: func [value] [valid? 'port! value]
rgb?:  func [value] [valid? 'rgb!  value]
slug?: func [value] [valid? 'slug! value]
```

Example generated constructor:

```rebol
port: func [p] [make port! p]
rgb:  func [r g b] [make rgb! reduce [r g b]]
slug: func [s] [make slug! s]
```

---

## Implementation Sketch

A minimal implementation needs:

1. A semantic type registry.
2. A component-extraction protocol (one extractor per supported base type).
3. A schema compiler that handles positional, scalar, streamed, and named shapes.
4. A validator.
5. Integration with function specs and dialect evaluators.

### Semantic Type Registry

```rebol
semantic-types: make map! []
```

### Component Extractors

```rebol
extractors: make map! [
    tuple!   func [v] [to-block v]
    pair!    func [v] [reduce [v/x v/y]]
    integer! func [v] [reduce [v]]
    number!  func [v] [reduce [v]]
    string!  func [v] [v]
    binary!  func [v] [v]
    block!   func [v] [v]
    url!     func [v] [form v]
    object!  func [v] [mold-object-pairs v]
    date!    func [v] [reduce [v/year v/month v/day]]
    time!    func [v] [reduce [v/hour v/minute v/second]]
]

to-components: func [value] [
    f: select extractors type? value
    either f [f value] [reduce [value]]
]
```

### Shape Detection

```rebol
shape-of: func [base] [
    case [
        find [tuple! pair! date! time!] base ['positional]
        find [integer! number!]         base ['scalar]
        find [string! binary! block! url!] base ['streamed]
        base = object!                     ['named]
        true                               ['scalar]
    ]
]
```

### Type Definition

```rebol
define-type: func [name base schema] [
    rule:  compile-schema schema shape-of base
    shape: shape-of base
    put semantic-types name make object! [
        name:       name
        base:       base
        shape:      shape
        schema:     schema
        parse-rule: rule
    ]
]
```

### Validation

```rebol
valid?: func [type value] [
    either builtin-type? type [
        type = type? value
    ][
        spec: select semantic-types type
        all [
            spec
            spec/base = type? value
            parse to-components value spec/parse-rule
        ]
    ]
]
```

### Generated Predicate

```rebol
port?: func [value] [valid? 'port! value]
rgb?:  func [value] [valid? 'rgb!  value]
slug?: func [value] [valid? 'slug! value]
```

This is only conceptual pseudocode, but it shows the basic architecture: extractors are pluggable, the schema compiler dispatches on shape, and the validator is uniform.

---

## Open Design Questions

### Should semantic types be first-class values?

Option A:

```rebol
rgb!
```

is a word registered in a global type registry.

Option B:

```rebol
rgb!: make semantic-type! [...]
```

is an actual first-class value.

First-class semantic types are more powerful, but global type words are simpler and closer to existing function specs.

### Should values carry semantic tags?

The untagged model is simpler. The tagged model preserves intent.

A hybrid model is probably best:

- ordinary literals are untagged
- constructors may optionally tag
- validators work either way

### Should semantic types participate in equality?

If two values have the same representation but different semantic tags, are they equal?

```rebol
make rgb! 1.2.3
make semver! 1.2.3
```

Possible answer:

```text
value equality: true
semantic equality: false
```

### Should overlapping semantic types be allowed?

Yes. The same raw value may satisfy multiple semantic schemas.

This is natural in a dynamic, context-driven language.

### How do object-backed semantic types differ from plain object prototypes?

A prototype defines structure and behavior. A semantic type over `object!` defines a *validation rule* over an object's fields, without requiring the object to have been constructed by any particular prototype. This lets semantic types validate objects produced by foreign code, serializers, or DSLs.

The two mechanisms can coexist: a prototype can satisfy a semantic type, and a semantic type can be used to validate arbitrary objects. The relationship is worth pinning down before shipping.

### Should parse failures expose captures?

For better errors, the schema compiler should preserve component names and expected constraints. Raw parse failure is not enough.

### How are streamed schemas indexed for error reporting?

Positional and named schemas have natural component names. Streamed schemas (string/block) do not. Error reporting for streamed schemas should use indices (`at index 3`) or token positions rather than field names.

---

## Recommended Initial Scope

A practical first version should be **generic in its foundation**, not specific to any base datatype. It should support:

- A **pluggable component-extraction protocol** with extractors for a starter set of base datatypes.
- Starter extractors for: `tuple!`, `pair!`, `integer!`, `number!`, `string!`, `block!`.
- All four schema shapes: positional, scalar, streamed, named (named is optional in v1 — see below).
- Named positional components (for tuple/pair).
- Single-component scalar schemas (for integer/number) with `range` and `where`.
- Streamed schemas with `some`, `any`, `optional`, and primitive character/token constraints.
- Primitive constraints: `integer`, `byte`, `positive-integer`, `non-negative-integer`, `number`, `slug-char`, `alpha`, `digit`, `hex-char`, `segment`.
- Generated predicates and constructors.
- Function spec validation.
- Simple, named error messages.

Example initial syntax:

```rebol
type rgb!:    tuple!   [r: byte  g: byte  b: byte]
type ipv4!:   tuple!   [a: byte  b: byte  c: byte  d: byte]
type size2d!: pair!    [width: positive-integer  height: positive-integer]
type port!:   integer! [range 1 65535]
type percent!: number! [range 0 100]
type slug!:   string!  [some slug-char]
type path!:   block!   [some segment]
```

Then expand later to:

- `object!` extractor and named schemas.
- `url!`, `date!`, `time!`, `binary!` extractors.
- Optional positional components.
- Repetition counts (e.g. `3 segment`).
- Dependent constraints (`range 1 days-in-month year month`).
- Tagged semantic values.
- Documentation generation.
- Editor tooling.
- Cross-field validation for object-backed types.

The principle is: **the extraction protocol is the MVP**. Once a base type has an extractor, every schema feature works for it automatically.

---

## Design Principle

The main design principle is:

```text
A raw datatype describes representation.
A semantic type describes intent.
A parse-backed schema connects the two — over any base type's component view.
```

So:

```rebol
255.0.0
```

is represented as `tuple!` but may be accepted as `rgb!`.

```rebol
192.168.1.10
```

is represented as `tuple!` but may be accepted as `ipv4!`.

```rebol
8080
```

is represented as `integer!` but may be accepted as `port!`.

```rebol
"user-42"
```

is represented as `string!` but may be accepted as `slug!`.

```rebol
[a b c]
```

is represented as `block!` but may be accepted as `path!`.

This approach fits a modern Rebol clone because it treats types as dialect-driven validators over any value's structure, rather than only as compiler-level declarations or only as tuple/pair special cases.

---

## Summary

Parse-backed semantic types provide a lightweight, Rebol-native way to add domain meaning to compact literal values, across **every** base datatype that can expose a component view.

They allow code like this:

```rebol
paint   255.0.0
connect 192.168.1.10 443
resize  window 800x600
navigate "user-42"
open    443
fetch   http://example.com
```

while still giving APIs and dialects the ability to enforce:

```rebol
paint:    func [color [rgb!]]                       [...]
connect:  func [address [ipv4!] port [port!]]       [...]
resize:   func [target [object!] size [size2d!]]    [...]
navigate: func [s [slug!]]                          [...]
open:     func [p [port!]]                          [...]
fetch:    func [u [http-url!]]                      [...]
```

The result is a type system that stays close to Rebol's philosophy:

- values are simple
- syntax is compact
- meaning comes from context
- dialects are central
- `parse` is the engine of structure
- any base datatype can host semantic subtypes via a pluggable extractor

In short:

> Semantic types are schemas over values, schemas are dialects that compile to parse rules, and every base datatype participates through its component view.
