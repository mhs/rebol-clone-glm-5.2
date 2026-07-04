Red []
; hash! is a series — foreach destructuring + pick/poke + append.
h: make hash! [a 1 b 2]
out: copy []
foreach [k v] h [append out v]
print out
out2: copy []
foreach [k v] h [append out2 k]
print out2
; append a key/value pair
append h [c 3]
print h/c
print length? h
; poke at the value slot (position 2 = first value)
poke h 2 99
print h/a
; clear
clear h
print length? h
