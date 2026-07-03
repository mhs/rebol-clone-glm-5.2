Red []
; M111: mold native — exposes printer.rs::mold_to_string to scripts.
; Each `print mold x` should produce the same text `probe` would (minus the
; `== ` prefix that `probe` adds).

print mold 5
print mold "hi"
print mold [1 2]
print mold 'word
print mold none
