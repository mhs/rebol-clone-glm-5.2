Red []
parse "a1b2" [collect w some [match #"a" | match #"b" | skip]]
probe w
