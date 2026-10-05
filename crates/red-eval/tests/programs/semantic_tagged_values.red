Red []
; Semantic types end-to-end: define-type across shapes, tagged values
; from make <type>!, the M175 discriminator, and M178 schema extensions.
define-type 'rgb! 'tuple! [r: byte g: byte b: byte]
define-type 'port! 'integer! [range 1 65535]
define-type 'code! 'string! [2 to 5 alpha]
define-type 'iso-date! 'tuple! [year: integer month: range 1 12 day: range 1 (days-in-month year month)]

; Generated predicates + constructors (constructors stay untagged).
; (rgb!'s byte constraint holds for every constructible tuple — the
; constructor rejects out-of-range components at construction time — so
; the false-predicate path shows via valid? instead. NOTE: `port?` is
; the builtin port! type predicate — the semantic predicate for port!
; collides with it and is not registered, so use `valid?`.)
print rgb? 1.2.3
print valid? 'port! 443
print valid? 'port! 70000
print code? "abc"
print code? "a1"

; make <type>! returns a TAGGED value: transparent everywhere,
; discriminable via semantic-type?.
t: make rgb! 1.2.3
print type? t
print semantic-type? t
print mold t
print mold/tagged t
print t = 1.2.3
print rgb? t
print semantic-type? rgb 1 2 3

; make with invalid value raises the rich error (caught for the fixture).
print mold try [make port! 70000]

; Dependent constraint: leap-year-aware day range.
print valid? 'iso-date! 24.2.29
print valid? 'iso-date! 23.2.29
print valid? 'iso-date! 23.2.28

; days-in-month helper (the constraint's engine).
print days-in-month 2024 2
print days-in-month 2023 2
print days-in-month 2000 2
print days-in-month 1900 2
