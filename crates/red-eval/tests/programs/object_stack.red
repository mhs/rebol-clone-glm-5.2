Red []
stack: make object! [
    items: []
    push: func [x][insert items x]
    pop: does [
        if empty? items [return none]
        take items
    ]
    peek: does [
        if empty? items [return none]
        first items
    ]
    size?: does [length? items]
    clear: does [clear items]
]

stack/push 10
stack/push 20
stack/push 30
print stack/size?
print stack/peek
print stack/pop
print stack/pop
print stack/size?
print stack/peek
stack/push 99
print stack/peek
print stack/pop
print stack/pop
print stack/size?
