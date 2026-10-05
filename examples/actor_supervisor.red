; M47: supervisor actor with link/monitor — the supervisor links to a
; child actor; when the child dies (handler error), the supervisor
; receives an "exit" message and actually restarts the child by calling
; spawn-actor + link. Then we verify the restarted child is alive.

Red []

; Define the child handler as a reusable func (so the supervisor can
; create a new child with the same behavior on restart).
child-handler: func [msg] [
    either msg = 'crash [
        print "child: crashing now"
        1 / 0
    ][
        print "child: alive"
    ]
]

; Supervisor: on "exit", restart the child.
sup: spawn-supervisor [func [msg] [
    either msg = "exit" [
        print "supervisor: child died, restarting..."
        child: spawn-actor [:child-handler]
        link sup child
        print "supervisor: child restarted"
    ][
        print "supervisor: ok"
    ]
]]

; Create the initial child and link it.
child: spawn-actor [:child-handler]
link sup child

; Crash the child.
send-actor child 'crash
run-actors

; The supervisor restarted the child. Verify it's alive.
send-actor child "hello"
run-actors
