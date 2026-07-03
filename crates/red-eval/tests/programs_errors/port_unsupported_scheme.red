Red []
; Reserved-but-unimplemented schemes are gated by `--allow-network` just like
; http/https — the golden runner runs with the gate closed, so this asserts
; the gate message. The UnsupportedInV09 dispatch itself is covered by inline
; tests in net/mod.rs (which set allow_network=true).
open whois://example.com
