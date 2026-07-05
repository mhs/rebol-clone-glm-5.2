Red []
print map-each x [1 2 3 4 5] [x * x]
a: [1 2 3 4 5 6]
remove-each x a [even? x]
print a
print map-each [k v] [10 20 30 40] [v + k]
