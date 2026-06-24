Red []
; Environment variables: get-env, set-env, env.
; Reads an existing var (if set), sets a scratch var, lists all.

print "HOME is:"
print get-env "HOME"

; Set and read back a scratch variable.
set-env "REBOL_CLONE_DEMO" "from-red"
print "scratch var:"
print get-env "REBOL_CLONE_DEMO"

; `env` with no args returns a block of "KEY=value" strings.
print "env block length:"
print length? env

; Find our scratch entry in the env block.
foreach entry env [
    if find entry "REBOL_CLONE_DEMO" [
        print "found in env:"
        print entry
    ]
]

; Clean up.
set-env "REBOL_CLONE_DEMO" none
