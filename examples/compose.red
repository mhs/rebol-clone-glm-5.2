Red []
; compose + type predicates (Milestone 39)
;
; `compose` walks a block, evaluating only (...) paren expressions and leaving
; everything else verbatim — the classic Red template-builder. Refinements:
;   compose/deep   recurse into nested blocks
;   compose/only   don't splice a block result (wrap as a single value)
;
; Plus the v0.4 type-predicate fill-in: integer?/float?/number?/string?/
; logic?/none?/any-word?/any-path?, `type?`, and `types-of`.

print "compose basics:"
print compose [a (1 + 2) b]                 ; [a 3 b]
print compose [foo "bar" 42 (1 + 1)]        ; [foo "bar" 42 2]
print compose [() (1) ()]                   ; [none 1 none]  (empty paren -> none)

print "compose/deep recurses into nested blocks:"
print compose/deep [a [(1 + 2)] b]          ; [a [3] b]
print compose/deep [outer [inner (2 * 3)] tail]

print "compose/only keeps a block result as a single value:"
print compose [([1 2 3])]                  ; [1 2 3]   (block result spliced)
print compose/only [([1 2 3])]             ; [[1 2 3]] (kept whole)

print "type predicates (the v0.4 fill-in):"
print integer? 5                            ; true
print integer? 5.0                          ; false
print float? 5.0                            ; true
print number? 5                             ; true  (int OR float)
print number? 5.0                           ; true
print string? "hi"                         ; true
print logic? true                          ; true
print none? none                           ; true
print any-word? first [foo]               ; true
print any-path? 'foo/bar                   ; true

print "type? (returns the type word):"
print type? 5                               ; integer!
print type? "hi"                            ; string!
print type? #"a"                           ; char!

print "types-of (all type words a value matches):"
print types-of 5                           ; [integer! number!]
print types-of 5.0                         ; [float! number!]
print types-of first [foo]                ; [word! any-word!]
print types-of [1 2]                       ; [block! any-block! series!]

; --- a practical use: build a config block from runtime values ---
config-name: "production"
config-port: 8080
cfg: compose [
    name: (config-name)
    port: (config-port)
    debug: (config-port = 8080)
]
probe cfg
