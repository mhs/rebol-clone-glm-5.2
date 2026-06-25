Red []
; Same accumulator via while — alt loop native.
acc: 0
i: 1
while [i <= 1000000] [
    acc: acc + i
    i: i + 1
]
print acc
