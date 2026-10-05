; M45: counter actor — increments on each message and replies with
; the current count. Demonstrates the actor pattern: spawn-actor,
; send-actor, reply via a per-actor reply channel.

Red []

reply: channel

a: spawn-actor [func [msg] [
    send reply msg + 1
]]

send-actor a 40
send-actor a 41
run-actors

print recv reply   ; 41
print recv reply   ; 42
