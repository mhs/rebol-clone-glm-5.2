Red []
print parse "abc def" [to " " skip copy rest to end]
print rest
print parse "abc" [thru "b" copy tail to end]
print tail
