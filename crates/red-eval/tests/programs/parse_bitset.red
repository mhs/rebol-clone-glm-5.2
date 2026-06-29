Red []
digits: charset "0123456789"
letters: charset "abcdefghijklmnopqrstuvwxyz"
print parse "x9y" [letters digits letters]
print parse "99" [some digits]
print extract? #"a" letters
print extract? #"1" letters
