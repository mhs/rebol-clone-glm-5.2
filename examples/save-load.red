Red []
; save / load: persist and reload Red values.
; `save` molds a value (reparseable form) to a file; `load` reads it back.
; Self-contained: writes a temp file, loads it, cleans up.

file: %examples/_tmp_save_demo.red

; Save a block of data.
save file [name: "Alice" age: 30 colors: [red green blue]]

; Load it back — returns a block wrapping the parsed body.
data: load file
body: first data

print "loaded body:"
probe body

print "name field:"
probe select body 'name

print "colors field:"
probe select body 'colors

; Save and reload a simple scalar.
save file 42
print "scalar reload:"
print first load file

delete file
