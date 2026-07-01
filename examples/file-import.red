Red []
; File-based module import (M62). `import %file.red` reads the file,
; evaluates it as a module body, caches it by canonical path, and
; aliases its exports into the current context.
;
; This demo creates a temp module file, imports it, and uses its
; exports. Run with: cargo run -p red-cli -- examples/file-import.red

; Write a small module file with data and function exports.
write %/tmp/demo_mod.red {
module 'demo [
    pi: 314
    greeting: "hello from module"
    nums: [1 2 3]
    dbl: func [x][x * 2]
    sq: func [x][x * x]
    export [pi greeting nums dbl sq]
]
}

; Import it — the file is read, loaded, and evaluated as a module.
; Its exports are aliased into the current context as bare words.
import %/tmp/demo_mod.red

; Data exports work in both VM and walk mode.
prin "pi = " print pi                ; 314
prin "greeting = " print greeting    ; hello from module
prin "nums = " print nums            ; [1 2 3]

; Function exports are callable bare (Bug 4 fix: the compiler falls back
; to the walker for blocks with unbound words in call position, so
; imported functions are dispatched dynamically like the walker does).
prin "dbl 21 = " print dbl 21        ; 42
prin "sq 7 = " print sq 7            ; 49

; The module is cached by canonical path — a second import doesn't
; re-read the file.
import %/tmp/demo_mod.red
prin "pi still = " print pi          ; 314

; Clean up the temp file.
delete %/tmp/demo_mod.red
print "done"
