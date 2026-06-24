Red []
n: 3
case/all [
    n > 1 [print "big"]
    n > 2 [print "bigger"]
    n > 5 [print "biggest"]
]
print "---"
n: 3
case [
    n > 5 [print "biggest"]
    n > 2 [print "bigger"]
    n > 1 [print "big"]
]
