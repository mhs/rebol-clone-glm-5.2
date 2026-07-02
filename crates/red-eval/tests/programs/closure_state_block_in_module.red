Red []
m: module [
    state: [0]
    bump: closure [][poke state 1 1 + pick state 1]
    get-count: closure [][pick state 1]
    export 'bump
    export 'get-count
]
m/bump
m/bump
m/bump
print m/get-count
