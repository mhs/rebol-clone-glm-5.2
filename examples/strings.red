Red []
; Strings: both "..." (with escapes) and {...} (multi-line, balanced braces).
; \"...\" honors \\ \" \n \t \r; braces preserve newlines and nest by depth.

print "quoted string with \"escapes\" and a tab:" print "a\tb"
print {braced string
spans multiple lines
and can contain "quotes" without escaping}
print {nested braces balance: {like this} and close}

; Strings are series of characters; treat them as data and mold them back.
greeting: {Hello, World!}
print greeting
probe greeting

; Build strings with rejoin (M15): reduce a block, form each result,
; concatenate with no separator. Use prin to write without a trailing newline.
prin "no trailing newline here -> "
prin "joined"
print " <- end"

; rejoin evaluates expressions and concatenates the formed results.
print rejoin ["user-" 42 "@example.com"]

; String escapes round-trip through mold.
probe "tab\there\nnewline"

; --- Milestone 15 string manipulation natives ---

; `+` concatenates two strings.
print "abc" + "def"               ; "abcdef"

; `join` forms both operands and concatenates (works across types).
print join "item-" 7              ; "item-7"

; `split` divides a string at each occurrence of a delimiter.
foreach part split "a,b,c" "," [
  print part
]

; `trim` strips whitespace; refinements: /all /lines /with /auto.
print trim "  padded  "           ; "padded"
print trim/all "  a  b  c  "      ; "abc"

; `replace` substitutes a substring; /all replaces every occurrence.
print replace "a-a" "a" "b"       ; "b-a"
print replace/all "a-a" "a" "b"   ; "b-b"

; `uppercase` / `lowercase` change case; /part limits the count.
print uppercase "hello"           ; "HELLO"
print uppercase/part "hello" 2    ; "HEllo"
print lowercase "WORLD"           ; "world"

; `find` on a string does substring search; returns the tail from the
; match position (POC approximation of Red's positioned string), or none.
probe find "hello" "ll"           ; == "llo"
probe find "hello" "zz"           ; == none

; `copy` on a string returns a fresh copy; /part limits the length.
print copy "abcdef"               ; "abcdef"
print copy/part "abcdef" 3        ; "abc"

; `suffix?` returns the file extension (including the dot), or none.
probe suffix? "report.txt"        ; == ".txt"
probe suffix? "README"            ; == none
