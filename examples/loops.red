Red []
; Loops: repeat, until, while, loop with break/continue
print "repeat:"
repeat i 3 [print i]

print "until:"
i: 0 until [i: i + 1 i > 3]
print i

print "while:"
a: 0 while [a < 3][a: a + 1]
print a

print "loop with break:"
i: 0 loop [i: i + 1 if i > 3 [break]]
print i

print "continue skips 3:"
repeat i 5 [if i = 3 [continue] print i]
