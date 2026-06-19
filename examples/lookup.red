Red []
; Table lookup with select and find
ages: [alice 30 bob 25 carol 41]
; select returns the value AFTER the matched key
print select ages 'bob
; find returns a positioned series at the match
print find ages 'carol
print first find ages 'carol
