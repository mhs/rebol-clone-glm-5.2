Red []
; Word kinds: Red distinguishes four word forms by prefix/suffix.
;   foo      Word      — evaluates to its bound value
;   foo:     SetWord   — assigns the next evaluated value
;   :foo     GetWord   — returns the word's value without calling it
;   'foo     LitWord   — the literal word itself, never evaluated

x: 42

print "Word evaluates to value:"
print x

print "SetWord assigns, expression continues:"
x: x + 1
print x

print "GetWord returns the value, useful for passing functions by name:"
inc: func [n][n + 1]
print inc :x

print "LitWord is a value (the word itself):"
probe 'foo
probe 'x

; LitWords are how you pass a word as a label rather than looking it up.
; select and find both take lit-words as keys.
table: [apple 1 banana 2 cherry 3]
print select table 'banana

; foreach binds a word to each value; using a lit-word keeps the iteration
; variable out of the user context.
foreach 'item [a b c][print item]

; set-word inside a block is data until the block is do'd.
spec: [target: 99]
do spec
print target

; Mixing word kinds in a block round-trips through mold.
probe [foo foo: :foo 'foo]