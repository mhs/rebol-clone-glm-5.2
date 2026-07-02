Red []
; M46 bitset set-ops still work after the M112 dispatcher rewrite
; (union/intersect/difference now route through series.rs's dispatcher,
; which delegates to the bitset helpers when both operands are bitset!).
print union charset "AB" charset "CD"
print intersect charset "ABCD" charset "BE"
print difference charset "ABCD" charset "BC"
print complement charset "A"
print extract? #"a" charset "abc"
print extract? #"z" charset "abc"
