Red []
; Real-world parse dialect demo: tokenize a small CSV string into a flat
; block of field strings. Uses the POC parse matcher subset: copy, to, skip,
; some, end, and paren side-effects.
;
; Note: the POC's parse does not evaluate words in rule position, so we
; match against a literal "\n" (newline escape) rather than the `newline`
; constant. Each row is newline-terminated (including the last) so the rule
; set is uniform.

csv: {Alice,30,red
Bob,25,green
Carol,41,blue
}

records: []
field: ""

; Walk the string: copy text up to a comma, skip the comma, append the field;
; repeat three times per row, then advance past the newline. `some` runs
; until the rule fails (at EOF).
parse csv [
    some [
        copy field to {,} skip (append records field)
        copy field to {,} skip (append records field)
        copy field to "\n" skip (append records field)
    ]
    end
]

; records is now a flat block of 9 strings (3 rows x 3 fields).
print "Parsed records:"
print records

; Print each field on its own line (print molds strings with quotes — a POC
; divergence from Red's form-based printing; use `prin form <value>` to
; emit raw text).
print "One per line:"
foreach 'f records [print f]