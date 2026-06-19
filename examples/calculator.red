Red []
; A small calculator demo. Note: `take` (and thus a `pop` built on it)
; removes from the HEAD of the series (the cursor sits at index 0 by
; default), so this behaves as a FIFO queue, not a LIFO stack. We use that
; deliberately to demonstrate the cursor-based series model.

queue: []

push: func [v][append queue v]
pop: does [
    either empty? queue [none][take queue]
]

; Enqueue 3, 4, 5.
push 3
push 4
push 5

; Dequeue two and multiply: (first) 3 * (second) 4 = 12, then enqueue 12.
push (pop) * pop
; Dequeue two and add: 5 + 12 = 17, then enqueue 17.
push (pop) + pop
print "result:"
print first queue

; Reset and run a longer sequence.
clear queue
foreach n [10 20 30][push n]
; Dequeue in FIFO order: 10, then 20, then 30.
print "dequeue all:"
print pop
print pop
print pop

; Show the queue as a block of values.
print "queue contents:"
probe queue