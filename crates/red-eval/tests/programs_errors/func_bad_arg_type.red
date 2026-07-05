Red []
; A func with [x [integer!]] rejects a string argument at call time.
f: func [x [integer!]][x + 1]
f "hi"
