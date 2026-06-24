Red []
; Reading files line-by-line: read/lines, foreach, length?.
; Writes a temp file, processes it line by line, cleans up.

file: %examples/_tmp_lines_demo.txt
write/lines file ["apple" "banana" "cherry" "date"]

lines: read/lines file
print "line count:"
print length? lines

print "each line uppercased:"
foreach line lines [
    print uppercase line
]

print "lines longer than 5 chars:"
foreach line lines [
    if find line "banana" [
        print line
    ]
]

; `save` molds a value to a file; `load` reads it back.
save file [1 2 3]
loaded: load file
print "saved+loaded block:"
print first loaded

delete file
