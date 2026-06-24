Red [title: "Paths"]

; block/integer path select
b: [10 20 30]
print b/2
print b/-1

; object field access + set-path
o: make object! [a: 1 b: 2]
print o/a
o/a: 99
print o/a

; nested object path
outer: make object! [
    inner: make object! [x: 42]
]
print outer/inner/x

; path with paren part (evaluated as index)
print b/(1 + 2)

; get-path returns function without calling
o2: make object! [f: does [42]]
print function? :o2/f

; lit-path returns as data
print lit-path? 'foo/bar

; to-path conversions
print to-path [a b c]
print to-get-path [x y]
print to-lit-path [p q]

; path predicates
print path? to-path [a b]
print get-path? to-get-path [a b]
