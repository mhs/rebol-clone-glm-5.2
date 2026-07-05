Red []
print make duration! 30
print make duration! 1.5
print make duration! "1.5h"
print make duration! "1d1h"
print make duration! "250ms"
print make duration! "-5m"
print make duration! [1 30 0]
print make duration! [0 1 30 0 0]
print make duration! [90]
print to-duration 45
print to-duration "2h"
print mold make duration! "1d1h"
print type? make duration! 30
print duration? to-duration "250ms"
