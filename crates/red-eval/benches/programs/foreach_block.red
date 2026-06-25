Red []
; foreach over a 100k block — series iteration.
data: copy []
repeat i 100000 [append data i]
acc: 0
foreach x data [acc: acc + x]
print acc
