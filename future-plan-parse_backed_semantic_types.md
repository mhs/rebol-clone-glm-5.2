# Parse-Backed Semantic Types for a Modern Rebol/Red Clone

## Overview

This document describes an approach for adding **semantic subtypes** to a Rebol/Red-style language while preserving the language’s core strengths: compact literal syntax, dialects, dynamic values, and `parse` as a general-purpose grammar engine.

The central idea is simple:

> Keep primitive datatypes like `tuple!`, `pair!`, `string!`, and `block!` broad and flexible, but allow semantic types to be defined as schemas backed by parse rules.

For example, both of these values are raw `tuple!` values:

```rebol
255.0.0
192.168.1.10
```

But in different contexts they mean different things:

```rebol
paint 255.0.0          ; RGB color
connect 192.168.1.10   ; IPv4 address
```

A parse-backed semantic type system lets us define and validate those meanings explicitly:

```rebol
type rgb!: tuple! [
    r: byte
    g: byte
    b: byte
]

type ipv4!: tuple! [
    a: byte
    b: byte
    c: byte
    d: byte
]
```

The values remain ordinary tuples at runtime, but functions, dialects, and APIs can require more specific semantic shapes.

---

## Motivation

Rebol and Red already provide compact domain-oriented literals:

| Literal | Raw Type | Common Meaning |
|---|---|---|
| `255.0.0` | `tuple!` | RGB color |
| `192.168.1.10` | `tuple!` | IPv4 address |
| `1.2.3` | `tuple!` | Version |
| `100x50` | `pair!` | Size, coordinate, or offset |
| `user@example.com` | `email!` in Rebol-style systems | Email address |

The challenge is that a raw datatype often does not fully express the intended meaning.

For instance:

```rebol
255.0.0
```

could be:

- an RGB color
- a semantic version
- a protocol version
- a compact numeric identifier

Likewise:

```rebol
192.168.1.10
```

is structurally similar to a four-component tuple, but semantically it is often an IPv4 address.

A raw type check can answer this:

```rebol
tuple? 255.0.0
; true
```

But it cannot answer this directly:

```rebol
rgb? 255.0.0
; true

ipv4? 255.0.0
; false
```

Parse-backed semantic types provide that second layer.

---

## Design Goals

The design should:

1. Preserve Rebol’s compact literal syntax.
2. Avoid turning every domain concept into a heavyweight object.
3. Allow semantic constraints such as arity, ranges, component names, and optional parts.
4. Work naturally inside dialects.
5. Use `parse` or a parse-compatible schema dialect as the underlying validation engine.
6. Provide readable error messages.
7. Allow both dynamic validation and optional static/tooling analysis.
8. Remain simple enough to fit Rebol’s philosophy.

The goal is not to replace `tuple!` with a rigid type hierarchy. The goal is to let specific contexts say:

> I accept a tuple, but only one that conforms to this semantic schema.

---

## Core Concept

A semantic type has three parts:

```rebol
type rgb!: tuple! [
    r: byte
    g: byte
    b: byte
]
```

This defines:

| Part | Meaning |
|---|---|
| `rgb!` | Name of the semantic type |
| `tuple!` | Underlying base datatype |
| schema block | Shape and constraints over the value |

The raw value is still a tuple:

```rebol
red: 255.0.0

type? red
; tuple!
```

But semantic predicates can be generated:

```rebol
rgb? red
; true

ipv4? red
; false
```

Function specs can then use semantic types:

```rebol
paint: func [
    color [rgb!]
] [
    ; ...
]
```

Now this is valid:

```rebol
paint 255.0.0
```

but this is rejected:

```rebol
paint 192.168.1.10
; error: expected rgb!, got tuple! with 4 components
```

---

## Why Use `parse`?

Rebol’s `parse` dialect is already a grammar system for recognizing structure in values. It can parse strings, blocks, and in a modern clone could be generalized or layered to parse the component representation of other values.

A tuple can be viewed as a sequence of components:

```rebol
to-block 255.0.0
; [255 0 0]

to-block 192.168.1.10
; [192 168 1 10]
```

Once exposed as a block, standard parse-style rules can validate it:

```rebol
byte-rule: [
    set n integer! (
        unless all [n >= 0 n <= 255] [fail]
    )
]

rgb-rule: [
    byte-rule byte-rule byte-rule end
]

ipv4-rule: [
    byte-rule byte-rule byte-rule byte-rule end
]
```

Then predicates can be implemented conceptually as:

```rebol
rgb?: func [value] [
    all [
        tuple? value
        parse to-block value rgb-rule
    ]
]

ipv4?: func [value] [
    all [
        tuple? value
        parse to-block value ipv4-rule
    ]
]
```

This makes `parse` the validation engine underneath semantic types.

---

## Public Syntax vs Internal Parse Rules

Although raw parse rules are powerful, they may not be ideal as the public type-definition syntax.

A nicer public schema dialect could be used:

```rebol
type rgb!: tuple! [
    r: byte
    g: byte
    b: byte
]
```

Internally, that could compile to a parse rule like:

```rebol
[
    set r byte-rule
    set g byte-rule
    set b byte-rule
    end
]
```

This keeps the user-facing syntax clean while still using `parse` internally.

In other words:

```text
schema dialect → parse rule → runtime validator
```

This gives the language a Rebol-native type mechanism without requiring a completely separate type-checking language.

---

## Example: RGB and RGBA

RGB and RGBA are good examples because they share the same base type but differ in arity.

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
```

Usage:

```rebol
red: 255.0.0
transparent-red: 255.0.0.128

rgb? red
; true

rgba? red
; false

rgba? transparent-red
; true
```

A drawing dialect can require these semantic types:

```rebol
draw [
    fill rgb! 255.0.0
    stroke rgba! 0.0.0.128
]
```

Or the expectation can be implied by keywords:

```rebol
draw [
    fill 255.0.0
    stroke 0.0.0.128
]
```

The dialect evaluator knows:

```text
fill   expects rgb!
stroke accepts rgb! or rgba!
```

---

## Example: IPv4

IPv4 can also be represented as a four-component tuple:

```rebol
type ipv4!: tuple! [
    a: byte
    b: byte
    c: byte
    d: byte
]
```

Usage:

```rebol
server: 192.168.1.10

ipv4? server
; true

rgb? server
; false
```

A networking API could use it directly:

```rebol
connect: func [
    address [ipv4!]
    port [integer!]
] [
    ; ...
]

connect 192.168.1.10 443
```

Invalid examples:

```rebol
connect 255.0.0 443
; error: expected ipv4!, got tuple! with 3 components

connect 999.1.2.3 443
; error: component a must be byte, got 999
```

Depending on the base `tuple!` rules, a value like `999.1.2.3` may not even be representable. But the schema should still define the intended range so the semantic type remains explicit and portable.

---

## Example: Semantic Version

Semantic versions also look like three-component tuples, but their meaning is different from RGB.

```rebol
type semver!: tuple! [
    major: integer
    minor: integer
    patch: integer
]
```

Usage:

```rebol
version: 1.4.2

semver? version
; true

rgb? version
; true, structurally, if rgb! only checks byte byte byte
```

This exposes an important design issue: some semantic types may overlap structurally.

The value `1.4.2` can be both a valid `semver!` and a valid `rgb!` if `rgb!` simply means “three byte-sized values.”

That is not necessarily a problem. It reflects a deeper principle:

> Semantic type validity does not always imply semantic intent.

In function arguments and dialects, the expected type provides the intent:

```rebol
paint 1.4.2      ; accepted if rgb!, but perhaps suspicious
require 1.4.2    ; accepted as semver!
```

Optional tooling could warn about ambiguous uses, but the runtime should usually validate against the expected semantic type.

---

## Example: Pair Subtypes

The same approach works for `pair!`.

A raw pair:

```rebol
100x50
```

could mean:

- a size
- a point
- an offset
- a grid coordinate

Define semantic pair types:

```rebol
type size2d!: pair! [
    width: positive-integer
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

size2d? button-size
; true

point2d? position
; true

offset2d? movement
; true
```

In a UI dialect:

```rebol
view [
    at 20x30
    button "Save" size 100x50
]
```

The evaluator can interpret pairs semantically by keyword:

```text
at   expects point2d!
size expects size2d!
```

---

## Function Specs with Semantic Types

A key use case is function argument validation.

```rebol
paint: func [
    color [rgb!]
] [
    ; ...
]

connect: func [
    address [ipv4!]
    port [integer!]
] [
    ; ...
]

resize: func [
    target [object!]
    size [size2d!]
] [
    ; ...
]
```

These specs are more expressive than raw base types:

```rebol
paint: func [color [tuple!]] [...]
connect: func [address [tuple!]] [...]
resize: func [size [pair!]] [...]
```

The raw specs say only what the representation is.

The semantic specs say what the value means.

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
    ]
]
```

The dialect grammar could declare expectations:

```rebol
ui-schema: [
    window string! 'size size2d! block!
    at point2d!
    button string! 'size size2d!
    box size2d! 'color rgb!
]
```

Or internally, a parser could use semantic validators:

```rebol
parse-ui: func [tokens] [
    parse tokens [
        some [
            'at set p pair! (validate point2d! p)
          | 'size set s pair! (validate size2d! s)
          | 'color set c tuple! (validate rgb! c)
          | skip
        ]
    ]
]
```

This keeps dialects readable while adding meaningful validation.

---

## Type Definition Model

A semantic type definition can be represented as metadata:

```rebol
make semantic-type! [
    name: 'rgb!
    base: tuple!
    schema: [
        r: byte
        g: byte
        b: byte
    ]
    parse-rule: [...compiled...]
]
```

The runtime registry might contain:

```rebol
semantic-types: make map! [
    rgb!   make semantic-type! [...]
    rgba!  make semantic-type! [...]
    ipv4!  make semantic-type! [...]
    size2d! make semantic-type! [...]
]
```

A generic validator can then work like this:

```rebol
valid?: func [type value] [
    spec: select semantic-types type

    all [
        spec/base = type? value
        parse components-of value spec/parse-rule
    ]
]
```

Where `components-of` converts base values into parseable sequences:

```rebol
components-of 255.0.0
; [255 0 0]

components-of 100x50
; [100 50]
```

---

## Component Extraction

Each base datatype needs a component extraction strategy.

| Base Type | Component Representation |
|---|---|
| `tuple!` | block of tuple components |
| `pair!` | block of `[x y]` |
| `string!` | character stream or original string |
| `block!` | original block |
| `object!` | field/value map or object accessor view |

Examples:

```rebol
components-of 255.0.0
; [255 0 0]

components-of 192.168.1.10
; [192 168 1 10]

components-of 100x50
; [100 50]
```

This gives the parse engine a consistent model:

```text
base value → component view → parse rule
```

---

## Primitive Constraint Types

Semantic schemas need a small vocabulary of primitive constraints.

Examples:

```rebol
byte              ; integer from 0 to 255
integer           ; any integer
positive-integer  ; integer greater than 0
nonzero-integer   ; integer except 0
number            ; integer or decimal
percent           ; number from 0 to 100
alpha             ; alphabetic character
slug-char         ; letter, digit, or hyphen
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

The schema dialect can hide this detail.

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

The validator can report:

```text
Invalid ipv4!: expected 4 components, got 3
```

or:

```text
Invalid ipv4!: component c must be byte, got 300
```

or:

```text
Invalid rgb!: expected component b to be byte, got "green"
```

The named fields in the schema are valuable even if the runtime representation is positional.

They allow the language to explain failures in domain terms.

---

## Constructors

Semantic types can generate constructors.

```rebol
rgb: func [r [byte!] g [byte!] b [byte!]] [
    make rgb! reduce [r g b]
]

ipv4: func [a [byte!] b [byte!] c [byte!] d [byte!]] [
    make ipv4! reduce [a b c d]
]
```

Usage:

```rebol
red: rgb 255 0 0
server: ipv4 192 168 1 10
```

The return values could still be raw tuples:

```rebol
type? red
; tuple!
```

Or the implementation could optionally tag them with semantic metadata:

```rebol
semantic-type? red
; rgb!
```

This is an important design choice.

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
]
```

The dialect interpreter validates that `100x50` is a `size2d!` and `255.0.0` is an `rgb!`.

### 3. Constructor Time

```rebol
red: rgb 255 0 0
```

The constructor validates components before producing the value.

### 4. Assignment Time

A stricter language could allow typed bindings:

```rebol
red [rgb!]: 255.0.0
```

or:

```rebol
red: rgb! 255.0.0
```

This is more static-feeling and should probably be optional.

---

## Parse Rule Compilation

A schema block:

```rebol
[
    r: byte
    g: byte
    b: byte
]
```

can compile to a parse rule:

```rebol
[
    set r byte-rule
    set g byte-rule
    set b byte-rule
    end
]
```

A schema with optional elements:

```rebol
type version!: tuple! [
    major: integer
    minor: integer
    patch: optional integer
]
```

could compile to:

```rebol
[
    set major integer!
    set minor integer!
    opt [set patch integer!]
    end
]
```

A schema with repetition:

```rebol
type path!: block! [
    some segment
]
```

could compile to:

```rebol
[
    some segment-rule
    end
]
```

The schema dialect does not need to expose every feature of `parse` immediately. It can start small and grow.

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

type slug!: string! [
    some slug-char
]
```

A later version could support dependent validation:

```rebol
type date!: tuple! [
    year: integer
    month: range 1 12
    day: range 1 days-in-month year month
]
```

But that should not be required for the first design.

---

## Relationship to Existing Rebol/Red Type Specs

Traditional function specs might look like this:

```rebol
connect: func [
    address [tuple!]
    port [integer!]
] [
    ; ...
]
```

The proposed model extends this naturally:

```rebol
connect: func [
    address [ipv4!]
    port [port!]
] [
    ; ...
]
```

The function machinery checks:

1. Is `ipv4!` a built-in datatype?
2. If not, is it a registered semantic type?
3. If yes, validate the value against the semantic type’s base and parse rule.

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
]
```

Without semantic types, the dialect sees:

```text
string!
pair!
tuple!
```

With semantic types, the dialect can say:

```text
size expects size2d!
color expects rgb!
```

This gives dialect authors a shared validation framework instead of forcing every dialect to hand-roll checks.

---

## Serialization

Because semantic values are often represented as ordinary base values, serialization can stay simple.

```rebol
red: 255.0.0
server: 192.168.1.10
```

Serialized as data:

```rebol
[
    color 255.0.0
    host 192.168.1.10
]
```

The receiving context determines semantics.

For tagged semantic values, serialization may need optional annotations:

```rebol
make rgb! 255.0.0
make ipv4! 192.168.1.10
```

or:

```rebol
#rgb 255.0.0
#ipv4 192.168.1.10
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
- Warn about ambiguous tuple usage.

Example generated docs:

```text
rgb!
Base: tuple!
Components:
  r byte 0..255
  g byte 0..255
  b byte 0..255
```

Example generated predicate:

```rebol
rgb?: func [value] [valid? rgb! value]
```

Example generated constructor:

```rebol
rgb: func [r g b] [make rgb! reduce [r g b]]
```

---

## Implementation Sketch

A minimal implementation needs:

1. A semantic type registry.
2. A schema compiler.
3. A component extraction function.
4. A validator.
5. Integration with function specs and dialect evaluators.

### Semantic Type Registry

```rebol
semantic-types: make map! []
```

### Type Definition

```rebol
define-type: func [name base schema] [
    rule: compile-schema schema

    put semantic-types name make object! [
        name: name
        base: base
        schema: schema
        parse-rule: rule
    ]
]
```

### Component Extraction

```rebol
components-of: func [value] [
    case [
        tuple? value [to-block value]
        pair? value  [reduce [value/x value/y]]
        block? value [value]
        string? value [value]
        true [reduce [value]]
    ]
]
```

### Validation

```rebol
valid?: func [type value] [
    either builtin-type? type [
        type = type? value
    ] [
        spec: select semantic-types type
        all [
            spec
            spec/base = type? value
            parse components-of value spec/parse-rule
        ]
    ]
]
```

### Generated Predicate

```rebol
rgb?: func [value] [valid? 'rgb! value]
```

This is only conceptual pseudocode, but it shows the basic architecture.

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

### Should parse failures expose captures?

For better errors, the schema compiler should preserve component names and expected constraints.

Raw parse failure is not enough.

---

## Recommended Initial Scope

A practical first version should support:

- `tuple!` semantic types
- `pair!` semantic types
- named positional components
- primitive constraints like `integer`, `byte`, `positive-integer`, `number`
- generated predicates
- function spec validation
- simple error messages

Example initial syntax:

```rebol
type rgb!: tuple! [
    r: byte
    g: byte
    b: byte
]

type ipv4!: tuple! [
    a: byte
    b: byte
    c: byte
    d: byte
]

type size2d!: pair! [
    width: positive-integer
    height: positive-integer
]
```

Then expand later to:

- strings
- blocks
- objects
- optional components
- repetitions
- dependent constraints
- tagged semantic values
- documentation generation
- editor tooling

---

## Design Principle

The main design principle is:

```text
A raw datatype describes representation.
A semantic type describes intent.
A parse-backed schema connects the two.
```

So:

```rebol
255.0.0
```

is represented as:

```text
tuple!
```

but may be accepted as:

```text
rgb!
```

And:

```rebol
192.168.1.10
```

is represented as:

```text
tuple!
```

but may be accepted as:

```text
ipv4!
```

This approach fits a modern Rebol clone because it treats types as dialect-driven validators rather than only as compiler-level declarations.

---

## Summary

Parse-backed semantic types provide a lightweight, Rebol-native way to add domain meaning to compact literal values.

They allow code like this:

```rebol
paint 255.0.0
connect 192.168.1.10 443
resize window 800x600
```

while still giving APIs and dialects the ability to enforce:

```rebol
paint: func [color [rgb!]] [...]
connect: func [address [ipv4!] port [port!]] [...]
resize: func [target [object!] size [size2d!]] [...]
```

The result is a type system that stays close to Rebol’s philosophy:

- values are simple
- syntax is compact
- meaning comes from context
- dialects are central
- `parse` is the engine of structure

In short:

> Semantic types are schemas over values, and schemas are dialects that compile to parse rules.
