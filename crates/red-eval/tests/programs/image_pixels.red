Red []
img: make image! [1 1 [0 0 0 0]]
poke img 1 255.0.0.255
print img/1
img2: make image! [2 1 [0 0 0 0 0 0 0 0]]
poke img2 2 10.20.30.40
print img2/1
print img2/2
