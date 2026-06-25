Red []
; append into a block 10,000 times — series mutation.
blk: copy []
repeat i 10000 [append blk i]
print length? blk
