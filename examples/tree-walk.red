Red []
; Nested data + recursion: walk a family tree stored as nested blocks.
; Each node is a block: [name child1 child2 ...] where each child is itself
; a node. Demonstrates blocks as a tree data structure, recursive functions,
; and first/next/foreach navigation.
;
; We avoid `use` inside func bodies (the POC's func-local setwords shadow
; outer words, which makes nested scopes brittle). Instead each recursive
; helper takes all state as parameters.

tree: [
    alice
    [bob]
    [carol [dave] [eve]]
    [frank]
]

; Count the leaves (names with no children). A node is a leaf if everything
; after its name is empty. Otherwise sum the leaf counts of each child.
count-leaves: func [t][
    either empty? t [0][
        either empty? next t [1][
            (count-leaves first next t)
            + count-leaves-of-children next next t
        ]
    ]
]

; Sum count-leaves over a block of children (each child is a node block).
count-leaves-of-children: func [children][
    either empty? children [0][
        (count-leaves first children)
        + count-leaves-of-children next children
    ]
]

; Collect all names into a flat block by appending as we recurse.
names-so-far: []
all-names: func [t][
    either empty? t [[]][
        append names-so-far first t
        all-names-of-children next t
        names-so-far
    ]
]

all-names-of-children: func [children][
    either empty? children [names-so-far][
        all-names first children
        all-names-of-children next children
    ]
]

; Depth: longest path from root to a leaf. A leaf has depth 1; otherwise
; 1 + the max child depth.
depth: func [t][
    either empty? t [0][
        either empty? next t [1][
            1 + max-child-depth next t 0
        ]
    ]
]

max-child-depth: func [children best][
    either empty? children [best][
        d: depth first children
        next-best: either d > best [d][best]
        max-child-depth next children next-best
    ]
]

print "leaf count:"
print count-leaves tree

print "all names (flattened):"
probe all-names tree

print "depth:"
print depth tree