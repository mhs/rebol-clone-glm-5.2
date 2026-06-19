Red []
; Series mutation: insert/change/remove/take/clear all operate at the cursor.
; Aliases share storage, so mutations are visible through every view.

blk: [a b c d e]
print "starting block:"
print blk

; insert puts a value BEFORE the cursor and advances.
print "insert X at head:"
insert blk "X"
print blk

; change replaces the value at the cursor and advances.
print "change first to Z:"
change blk "Z"
print blk

; take removes and returns the value at the cursor.
print "take first:"
print take blk
print blk

; remove drops the value at the cursor (no return).
print "remove first:"
remove blk
print blk

; clear truncates from cursor to tail.
print "clear from cursor:"
clear blk
print blk

; Shared storage: mutations via an alias are visible through the original.
original: [1 2 3]
alias: original
append alias 4
insert alias 0
print "original after alias mutations:"
print original

; copy breaks the sharing link.
independent: copy original
append independent 99
print "original untouched after copy+append:"
print original
print "the copy:"
print independent

; Series-native summary: pick/poke by 1-based index.
table: [10 20 30 40 50]
print "pick index 3:"
print pick table 3
poke table 3 999
print "after poke index 3 -> 999:"
print table