Red []
; Parse-backed semantic types (Milestone 170+)
;
; A semantic type is a schema over a base datatype, compiled to a parse rule.
; The raw value stays its base type at runtime — `type? red` is `tuple!` —
; but predicates and function specs can enforce the semantic constraint.
;
; `define-type 'name 'base [schema]` compiles the schema and registers the
; type. `valid? 'name value` checks it. A predicate (`rgb?`) and constructor
; (`rgb`) are auto-generated for each type. `make <type>! <value>` is the
; standard Rebol construction form.

print "positional schema — tuple! (RGB color):"
define-type 'rgb! 'tuple! [r: byte g: byte b: byte]
print rgb? 255.0.0          ; true
print rgb? 192.168.1.10     ; false  (4 components, not 3)
print type? 255.0.0         ; tuple! (still its base type)

print "positional schema — tuple! (IPv4 address):"
define-type 'ipv4! 'tuple! [a: byte b: byte c: byte d: byte]
print valid? 'ipv4! 192.168.1.10    ; true
print valid? 'ipv4! 255.0.0         ; false  (only 3 components)

print "scalar schema — integer! (TCP port):"
define-type 'port! 'integer! [range 1 65535]
print valid? 'port! 443      ; true
print valid? 'port! 99999    ; false  (out of range)
print valid? 'port! "443"    ; false  (wrong base type)

print "scalar schema — number! (percentage):"
define-type 'percent! 'number! [range 0 100]
print valid? 'percent! 50      ; true
print valid? 'percent! 50.5    ; true   (number! accepts float!)
print valid? 'percent! 150     ; false

print "positional schema — pair! (2D size):"
define-type 'size2d! 'pair! [width: positive-integer height: positive-integer]
print valid? 'size2d! 100x50     ; true
print valid? 'size2d! -5x10      ; false  (negative width)

print "positional schema — pair! (with optional field):"
define-type 'coord! 'pair! [x: integer y: integer z: optional integer]
print valid? 'coord! 10x20       ; true   (z is optional, absent)

print "streamed schema — string! (URL slug):"
define-type 'slug! 'string! [some slug-char]
print valid? 'slug! "user-42"        ; true
print valid? 'slug! "Ada Lovelace"   ; false  (space not a slug-char)

print "streamed schema — string! (hex color):"
define-type 'hex-color! 'string! ["#" some hex-char]
print valid? 'hex-color! "#ff0000"    ; true
print valid? 'hex-color! "ff0000"     ; false  (missing #)

print "streamed schema — block! (path segments):"
define-type 'path! 'block! [some segment]
print valid? 'path! [a b c]       ; true
print valid? 'path! [1 2 3]       ; false  (integers aren't segments)

print "named schema — object! (person record):"
define-type 'person! 'object! [
    name: string
    age: optional [range 0 150]
]
print valid? 'person! make object! [name: "Ada" age: 36]     ; true
print valid? 'person! make object! [name: 123]                ; false  (name not string)
print valid? 'person! make object! [name: "Ada" age: 200]     ; false  (age out of range)
print valid? 'person! make object! [name: "Ada"]              ; true   (age is optional)

print "named schema — nested semantic types:"
define-type 'point2d! 'pair! [x: integer y: integer]
define-type 'rect! 'object! [origin: point2d! size: size2d!]
print valid? 'rect! make object! [origin: 20x30 size: 100x50]    ; true

print "generated constructors (validate before building):"
print rgb 255 0 0              ; 255.0.0  (builds a tuple! from components)
print type? rgb 255 0 0        ; tuple!
print slug "user-42"           ; "user-42"

print "make <type>! <value> (standard Rebol construction):"
red: make rgb! 255.0.0
print red                     ; 255.0.0
print type? red               ; tuple!  (still its base type)
print rgb? red                ; true    (semantic predicate)
p: make port! 443
print p                       ; 443
s: make slug! "user-42"
print s                       ; "user-42"
; make rgb! 192.168.1.10      ; error: Invalid rgb: expected 3 components, got 4
; make port! 99999             ; error: Invalid port: must be in range 1..65535, got 99999

print "validate (raises rich error on failure):"
print validate 'rgb! 255.0.0   ; 255.0.0  (returns value on success)
; validate 'port! 99999       ; error: Invalid port: must be in range 1..65535, got 99999

print "function spec validation:"
paint: func [color [rgb!]] [color]
print paint 255.0.0            ; 255.0.0  (valid RGB)
; paint 192.168.1.10          ; error: type error: arg 1 expected rgb! (base tuple!), got tuple!

connect: func [addr [ipv4!] p [port!]] [addr]
print connect 192.168.1.10 443    ; 192.168.1.10

navigate: func [s [slug!]] [s]
print navigate "user-42"          ; "user-42"

print "to-components (the extraction protocol):"
print to-components 255.0.0           ; [255 0 0]
print to-components 100x50            ; [100 50]
print to-components 8080              ; [8080]
print to-components make object! [name: "Ada" age: 36]  ; [name "Ada" age 36]

print "semantic-type! is a first-class value:"
t: make semantic-type! [name: 'rgb! base: 'tuple! schema: [r: byte g: byte b: byte]]
print semantic-type? t        ; true
print type? t                 ; semantic-type!
