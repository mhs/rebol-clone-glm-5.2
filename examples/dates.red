Red []
; date! / time! / now (Milestone 45)
;
; One `Value::Date` variant covers date-only, date+time, and date+time+zone.
; `time?` is true for dates with a time component or a non-None zone. Timezone
; model matches Red parity: fixed UTC offsets only (no named zones, no DST).
;
; Literal forms supported:
;   DD-Mon-YYYY                    29-Jun-2024           (zone-naive, date-only)
;   YYYY-MM-DD                     2024-06-29            (ISO form)
;   DD-Mon-YYYY/HH:MM:SS           29-Jun-2024/12:30:00  (with time)
;   YYYY-MM-DDTHH:MM:SSZ           2024-06-29T12:30:00Z  (ISO + UTC zone)
;   ...+HH:MM  -HH:MM  +HHMM  +HH  any of these zone suffixes
; /  is the date/time separator inside a literal; / is otherwise a path delim.

print "literals:"
print 29-Jun-2024                           ; 29-Jun-2024                  (date-only, zone-naive)
print 2024-06-29                           ; 29-Jun-2024                  (ISO form re-molds to the Red form)
print 2024-06-29T12:30:00Z                 ; 29-Jun-2024/12:30:00+00:00   (UTC)
print 29-Jun-2024/12:30:00+5:30           ; 29-Jun-2024/12:30:00+05:30
print 12:30:00-04:00                      ; 01-Jan-1970/12:30:00-04:00   (time-only)

print "predicates + type:"
print date? 29-Jun-2024                   ; true
print date? 5                             ; false
print time? 12:30:00                      ; true   (time-only value has a time component)
print time? 29-Jun-2024                   ; false  (date-only, no time)
print type? 29-Jun-2024                    ; date!

print "arithmetic:"
print 29-Jun-2024 + 1                     ; 30-Jun-2024  (date + N days; zone preserved)
print 29-Jun-2024 + 7                     ; 06-Jul-2024
print 30-Jun-2024 - 29-Jun-2024           ; 1            (date - date -> integer day difference)
d: 1-Jan-2024
print d + 365                            ; 31-Dec-2024  (crosses the year boundary)

print "field paths:"
d: 29-Jun-2024/12:30:00+5:30
print d/year                              ; 2024
print d/month                            ; 6
print d/day                             ; 29
print d/time                            ; 01-Jan-1970/12:30:00   (time as a time!-shaped date)
print d/weekday                         ; 6   (1=Monday ... 7=Sunday; Sat=6)
print d/yearday                        ; 181 (day-of-year, 1-indexed)

print "zone access (zone-naive dates return none):"
print d/zone                            ; 01-Jan-1970/05:30:00  (offset as a duration, time!-shaped)
naive: 29-Jun-2024
print naive/zone                         ; none

print "date/zone: set-path relabels (no shift):"
d: 29-Jun-2024/12:30:00+5:30
d/zone: -240                            ; -240 minutes = -04:00
print d                                 ; 29-Jun-2024/12:30:00-04:00  (same wall-clock, new label)

print "to-utc shifts + relabels to UTC:"
print to-utc 29-Jun-2024/12:30:00+5:30  ; 29-Jun-2024/07:00:00+00:00
print to-utc 29-Jun-2024/12:30:00-04:00 ; 29-Jun-2024/16:30:00+00:00
print to-utc 29-Jun-2024/12:30:00+00:00 ; 29-Jun-2024/12:30:00+00:00

print "now / today (current local time, zone = local UTC offset):"
print now/year >= 2024                  ; true
print date? now                        ; true
print time? now                        ; true   (now has a time component)
print today/year >= 2024               ; true
print date? today                       ; true
print time? today                      ; false  (today is date-only at local midnight)

print "to-date (from block, from epoch):"
print to-date [2024 6 29]               ; 29-Jun-2024
print to-date [2024 6 29 12 30 0]        ; 29-Jun-2024/12:30:00
print to-date 0                         ; 01-Jan-1970/00:00:00+00:00  (UTC epoch -> zone = 0)
