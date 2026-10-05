; M43: channel echo — main thread creates a channel, spawns a worker
; that loops on recv and echoes back via a second channel. Demonstrates
; bidirectional communication.

Red []

; Create two channels: request (main -> worker) and reply (worker -> main).
req: channel
reply: channel

; Spawn a worker that loops: recv on req, send the value back on reply.
; The worker closes its end when it receives none (channel drained).
spawn [
    loop [
        msg: recv req
        if none? msg [close reply exit]
        send reply msg
    ]
]

; Send a few messages.
send req "hello"
send req 42
send req [1 2 3]

; Collect echoes.
print recv reply        ; hello
print recv reply        ; 42
print mold recv reply   ; [1 2 3]

; Signal the worker to stop.
close req
