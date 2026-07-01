Red []
; main — end-to-end integration demo: imports the four sibling modules
; and exercises their exports. Run with:
;   cargo run -p red-cli -- examples/modules/main.red
; (or: cd examples/modules && red main.red)

import %mathutils.red
import %stringutils.red
import %tree.red
import %counter.red

; --- mathutils ---
prin "sq 5 = " print sq 5                ; 25
prin "cube 3 = " print cube 3            ; 27
prin "is-even? 4 = " print is-even? 4    ; true
prin "is-odd? 4 = " print is-odd? 4       ; false
prin "triangular 5 = " print triangular 5 ; 15

; --- stringutils ---
prin "capwords = " print capwords "hello world foo"  ; HELLO WORLD FOO
prin "snake-to-camel = " print snake-to-camel "foo-bar-baz"  ; fooBarBaz
prin "truncate-to = " print truncate-to "hello world" 5  ; hello...

; --- tree (BST) ---
root: bst-insert none 5
root: bst-insert root 3
root: bst-insert root 8
root: bst-insert root 1
root: bst-insert root 4
prin "pre-order = " print bst-walk-pre root    ; 5 3 1 4 8
prin "in-order = " print bst-walk-in root     ; 1 3 4 5 8

; --- counter (closures with encapsulated state) ---
; Module-level closures capture the module's ctx variables; writes
; persist across invocations (the RefCell cell mechanism).
prin "bump-a = " print bump-a       ; 1
prin "bump-a = " print bump-a       ; 2
prin "bump-a = " print bump-a       ; 3
prin "bump-b = " print bump-b       ; 101
prin "bump-a = " print bump-a       ; 4 (independent from bump-b)
prin "reset-a = " print reset-a     ; 0
prin "get-a = " print get-a         ; 0

prin "clamped = " print bump-clamped ; 1
prin "clamped = " print bump-clamped ; 2
prin "clamped = " print bump-clamped ; 3
prin "clamped = " print bump-clamped ; 3 (clamped at 3)
prin "reset-clamped = " print reset-clamped ; 0

print "done"
