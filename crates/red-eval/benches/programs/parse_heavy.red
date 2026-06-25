Red []
; parse over a 10k-char string — parse dialect overhead.
; (Expected VM-neutral: parse stays on the walker in v0.3.)
; `append` is series-only in this POC, so build the string via `rejoin`.
src: copy ""
repeat i 10000 [src: rejoin [src "a"]]
count: 0
parse src [some ["a" (count: count + 1)]]
print count
