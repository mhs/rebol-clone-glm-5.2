; M43: parallel fib — spawns 4 workers computing fib(30) each, collects
; results via recv on 4 result channels, prints the total. Demonstrates
; real parallelism (the 4 fib(30)s run on 4 cores concurrently).

Red []

fib: func [n] [
    either n < 2 [n] [fib n - 1 + fib n - 2]
]

; Spawn 4 workers, each computing fib(30).
r1: spawn [fib 30]
r2: spawn [fib 30]
r3: spawn [fib 30]
r4: spawn [fib 30]

; Collect results (blocks until each worker finishes).
v1: recv r1
v2: recv r2
v3: recv r3
v4: recv r4

; Print the total.
print v1 + v2 + v3 + v4
