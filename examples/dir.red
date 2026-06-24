Red []
; Directory operations: what-dir, dir?, make-dir, delete.
; Self-contained: creates a temp directory tree, checks it, removes it.

print "current directory:"
print what-dir

print "is it a directory?"
print dir? what-dir

; Create a nested temp directory under the current dir.
tmp: %examples/_tmp_dir_demo
nested: %examples/_tmp_dir_demo/sub/nested

make-dir nested
print "nested dir created?"
print dir? nested

print "parent exists?"
print exists? tmp

; Remove the whole tree (delete recurses on directories).
delete tmp
print "exists after delete?"
print exists? tmp
