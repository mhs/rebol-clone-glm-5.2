Red []
n: 0
parse "a;b;c" [some [skip (n: n + 1) to ";" skip] to end]
print n
