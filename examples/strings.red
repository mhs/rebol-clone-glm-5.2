Red []
; Strings: both "..." (with escapes) and {...} (multi-line, balanced braces).
; \"...\" honors \\ \" \n \t \r; braces preserve newlines and nest by depth.

print "quoted string with \"escapes\" and a tab:" print "a\tb"
print {braced string
spans multiple lines
and can contain "quotes" without escaping}
print {nested braces balance: {like this} and close}

; Strings are series of characters; treat them as data and mold them back.
greeting: {Hello, World!}
print greeting
probe greeting

; Build strings by appending to a block then reducing, since the POC has no
; rejoin yet (planned for v0.2). Use prin to write without a trailing newline.
prin "no trailing newline here -> "
prin "joined"
print " <- end"

; String escapes round-trip through mold.
probe "tab\there\nnewline"