Red []
classify: func [n][
    if n < 0 [return "negative"]
    if n = 0 [return "zero"]
    "positive"
]
print classify -5
print classify 0
print classify 7
