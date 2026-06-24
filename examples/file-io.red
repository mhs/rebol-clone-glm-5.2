Red []
; File I/O: write, read, exists?, size?, delete.
; Self-contained: writes a temp file, reads it back, then removes it.

file: %examples/_tmp_io_demo.txt

write file "line one
line two
line three"

print "exists after write?"
print exists? file

print "size in bytes:"
print size? file

print "contents:"
print read file

print "line count via read/lines:"
print length? read/lines file

print "first line:"
print first read/lines file

; Clean up.
delete file
print "exists after delete?"
print exists? file
