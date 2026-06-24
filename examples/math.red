Red []
; Math + bitwise (Milestone 17)

print "modulo:"
print 7 // 3
print 7.0 // 3.0
print 10 // 4

print "abs / negate:"
print abs -5
print negate 4
print abs -2.5

print "word aliases:"
print add 2 3
print subtract 10 4
print multiply 3 4
print divide 10 2

print "min / max:"
print min 3 5
print max 3 5
print min 3.5 2.5

print "round:"
print round 3.6
print round 3.4
print round/to 3.14159 0.01
print round/even 2.5
print round/even 3.5

print "power:"
print 2 ** 3
print power 2 10
print 2.0 ** 0.5
print 2 ** -1

print "even? / odd?:"
print even? 4
print even? 5
print odd? 5
print odd? 4

print "bitwise on integers:"
print 5 and 3
print 5 or 3
print 5 xor 3
print complement 0
print shift-left 1 3
print shift-right 8 2

print "logic and/or (unchanged):"
print true and false
print true or false
print true xor false

print "random:"
random/seed 99
print random 6
print random 1.0

; FizzBuzz using math + conditionals
print "fizzbuzz:"
repeat i 15 [
    case [
        i // 15 = 0 [print "FizzBuzz"]
        i // 3 = 0  [print "Fizz"]
        i // 5 = 0  [print "Buzz"]
        true        [print i]
    ]
]
