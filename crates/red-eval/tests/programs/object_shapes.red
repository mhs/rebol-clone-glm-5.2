Red []
shape: make object! [
    name: "shape"
    area: does [0]
    describe: does [rejoin [name " area=" area]]
]

circle: make object! [shape name: "circle" radius: 5 area: does [3 * radius * radius]]

rect: make object! [shape name: "rect" w: 4 h: 6 area: does [w * h]]

print circle/describe
print circle/area
print rect/describe
print rect/area
print words-of circle
print words-of rect
