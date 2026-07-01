; tree — a binary search tree module using closures for traversal.
; Demonstrates: closures capturing construction-time values, recursive
; closure calls, and closures composing with block data.

module 'tree [
    ; A node is a block: [value left right] where left/right are
    ; nodes (blocks) or none.
    make-node: func [v][reduce [v none none]]

    ; Insert `v` into a BST rooted at `root` (a node block or none).
    ; Returns the (possibly new) root.
    bst-insert: func [root v] [
        either none? root [
            make-node v
        ] [
            cur: first root
            either v < cur [
                poke root 2 (bst-insert (second root) v)
                root
            ] [
                poke root 3 (bst-insert (third root) v)
                root
            ]
        ]
    ]

    ; Pre-order walk: collect values [root left right] into a block.
    bst-walk-pre: func [root] [
        either none? root [copy []] [
            acc: copy []
            append acc (first root)
            append acc (bst-walk-pre (second root))
            append acc (bst-walk-pre (third root))
            acc
        ]
    ]

    ; In-order walk: collect values [left root right] — sorted order.
    bst-walk-in: func [root] [
        either none? root [copy []] [
            acc: copy []
            append acc (bst-walk-in (second root))
            append acc (first root)
            append acc (bst-walk-in (third root))
            acc
        ]
    ]

    export [make-node bst-insert bst-walk-pre bst-walk-in]
]
