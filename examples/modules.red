Red []
; Modules (module!) are self-contained namespaces with exported words.
; `module [body]` evaluates the body in the module's own context; `export`
; marks words as public. `import` aliases a module's exports into the
; current context so bare words resolve to them.

; --- anonymous module ---
; `module [body]` creates an anonymous module. SetWords inside the body
; populate the module's context (not the script's user_ctx). Only
; exported words are visible from outside via `module/word` paths.
m: module [
    priv: 42
    pub: 100
    export 'pub
]
prin "m/pub = " print m/pub          ; 100
; m/priv would error — private word from outside

; --- named module + import ---
; `module 'name [body]` creates a named module cached by name. Assign
; the returned value to a word for path access (`name/word`). `import
; 'name` aliases its exports into the current context as bare words.
mathx: module 'mathx [
    sq: func [x][x * x]
    cube: func [x][x * x * x]
    export [sq cube]
]
import 'mathx
prin "sq 5 = " print sq 5            ; 25
prin "cube 3 = " print cube 3        ; 27
; Also reachable via path (using the assigned word):
prin "mathx/sq 7 = " print mathx/sq 7   ; 49

; --- module with private helper ---
; Private words are visible inside the module body but not exported.
; The exported function can call the private helper internally.
stringsx: module 'stringsx [
    internal-sep: "_"            ; private constant
    join-with-sep: func [a b][
        rejoin [a internal-sep b]
    ]
    export 'join-with-sep
]
import 'stringsx
prin "join-with-sep \"foo\" \"bar\" = " print join-with-sep "foo" "bar"  ; foo_bar
; internal-sep is private — not aliased by import
print value? 'internal-sep    ; false

; --- module is a singleton by name ---
; Re-evaluating `module 'name [...]` returns the cached module; the new
; body is ignored. This matches Red's "module is a singleton by name".
once: module 'once [x: 1 export 'x]
once2: module 'once [x: 999]        ; body ignored — cached module returned
import 'once
prin "once/x = " print once/x       ; 1 (not 999)
prin "once2/x = " print once2/x     ; 1 (same cached module)

; --- import overwrites existing words ---
; If a word already exists in the script, `import` overwrites it with
; the module's exported value.
val: 0
module 'over [val: 42 export 'val]
import 'over
prin "val after import = " print val   ; 42 (overwritten)

; --- module? predicate ---
prin "module? m = " print module? m    ; true
prin "module? 5 = " print module? 5    ; false

; --- words-of returns exports only ---
; From outside, `words-of` shows only the public surface.
prin "words-of m = " print words-of m   ; [pub] (not [priv pub])
