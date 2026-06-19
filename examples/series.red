Red []
; Series navigation: cursor-based views over shared storage
blk: [10 20 30 40 50]
print first blk
print second blk
print third blk
print last blk
; next returns a positioned series at index+1; first of it is the 2nd element
print first next blk
; index? is 1-based; length? counts from cursor to tail
print index? next blk
print length? next blk
; at is absolute 1-based; skip is relative to the cursor
print first at blk 4
print first skip next blk 2
; back moves the cursor toward the head
print index? back next next blk
