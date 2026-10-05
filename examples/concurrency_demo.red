; Concurrency demo — showcases the full v0.6 concurrency stack:
; channels, spawn (OS threads), and cooperative actors.
;
; Run with: red-cli examples/concurrency_demo.red

Red []

; ============================================================
; 1. Channels — bidirectional, Go-style. Both ends in one value.
; ============================================================

print "--- Channels ---"

c: channel
send c 42
print recv c                        ; 42

; Objects cross the Send boundary (deep-cloned to independent storage).
c2: channel
send c2 make object! [name: "Alice" age: 30]
person: recv c2
print person/name                  ; Alice
print person/age                   ; 30

close c2
print closed? c2                    ; true

; ============================================================
; 2. Spawn — OS-thread workers with result channels.
; ============================================================

print "--- Spawn (parallel) ---"

; Simple arithmetic on worker threads.
r1: spawn [1 + 2 * 3]
r2: spawn [10 * 10]
print recv r1                       ; 9
print recv r2                       ; 100

; String result from a worker.
r3: spawn ["hello from worker"]
print recv r3                       ; hello from worker

; ============================================================
; 3. Actors — cooperative single-threaded scheduler.
; ============================================================

print "--- Actors ---"

; Simple actor that prints messages.
greeter: spawn-actor [func [msg] [
    print msg
]]

send-actor greeter "hello from actor"
send-actor greeter "world"
run-actors

; ============================================================
; 4. Actor with error handling + links.
; ============================================================

print "--- Actor links ---"

sup: spawn-supervisor [func [msg] [
    either msg = "exit" [
        print "supervisor: child died, would restart"
    ][
        print "supervisor: ok"
    ]
]]

child: spawn-actor [func [msg] [
    either msg = "crash" [
        print "child: crashing"
        1 / 0
    ][
        print "child: alive"
    ]
]]

link sup child
send-actor child "hello"
send-actor child "crash"
run-actors

print "--- Done ---"
