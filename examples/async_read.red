; M43: async read — spawn [read url!] returns immediately; the main
; thread does other work; recv result blocks until the read finishes.
; Demonstrates non-blocking I/O (the URL fetch runs on a worker thread
; while the main thread continues).
;
; Run with: red-cli --allow-network examples/async_read.red

Red []

; Start a URL fetch on a worker thread (returns immediately).
result: spawn [read http://example.com/]

; The main thread can do other work here while the fetch runs...
print "fetching..."

; Block until the fetch finishes, then print the response.
body: recv result
print body
