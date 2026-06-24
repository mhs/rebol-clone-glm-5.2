Red []
outer: make object! [
    inner: make object! [x: 1 y: 2]
    z: 3
]
outer/inner/x: 99
print outer/inner/x
print outer/inner/y
print outer/z
