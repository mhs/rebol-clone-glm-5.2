; Mini stdlib (M64, surfaced by M63 wiring).
;
; Compiled into the binary via `include_str!` and auto-imported into
; `user_ctx` at script startup (unless `--no-stdlib`). Re-exported bare
; words resolve via the M62 `resolve_word` Unbound → user_ctx fallback
; in the walker, and via `LoadDynamic` in the VM.
;
; Names are deliberately chosen to avoid collisions with existing
; examples/test fixtures: `square`/`cube`/`sum-of`/`fib`/`factorial` are
; NOT defined here (they appear as user-defined words in fixtures). `abs`
; is a native (handles pairs/tuples) so it's not re-defined here.
;
; String note: POC strings are immutable `Rc<str>` (not series-backed),
; so `length?`/`pick`/`skip`/`head` don't work on them directly. String
; utilities that need index/length access convert via `split s ""` (a
; block of chars) and use block operations, then `rejoin` back to a
; string. `find`/`copy/part`/`=` do work on strings directly.
;
; ~25 utilities across string/block/math/sort. Defer a full stdlib to v0.6.
; No `Red []` header: this source is `include_str!`-loaded via `load_source`
; (which doesn't strip a header), not parsed as a standalone program.

module 'stdlib [

    ; --- string utils ---

    str-upper: func [s] [uppercase s]
    str-lower: func [s] [lowercase s]
    ; Does `s` start with `prefix`? `find` returns a positioned string at
    ; the match; if the match is at index 0, the result equals `s`.
    starts-with?: func [s prefix] [
        (find s prefix) = s
    ]
    ; Does `s` end with `suffix`? Convert to char blocks, extract the
    ; tail of length `flen`, rejoin back to a string, compare.
    ends-with?: func [s suffix] [
        s-chars: split s ""
        suf-chars: split suffix ""
        slen: length? s-chars
        flen: length? suf-chars
        either slen < flen [false] [
            tail-chars: copy/part (skip s-chars (slen - flen)) flen
            (rejoin tail-chars) = suffix
        ]
    ]
    contains?: func [s sub] [
        not none? find s sub
    ]
    ; Join a block of strings with a separator.
    str-join: func [blk sep] [
        either empty? blk [""] [
            acc: copy (first blk)
            foreach s (next blk) [
                acc: rejoin [acc sep s]
            ]
            acc
        ]
    ]
    ; Repeat a string n times.
    repeat-str: func [s n] [
        acc: copy []
        i: 0
        while [i < n] [
            append acc s
            i: i + 1
        ]
        rejoin acc
    ]
    ; Pad `s` on the left with `pad` char until it reaches `width`.
    pad-left: func [s width pad] [
        acc: copy s
        while [(length? split acc "") < width] [
            acc: rejoin [pad acc]
        ]
        acc
    ]
    pad-right: func [s width pad] [
        acc: copy s
        while [(length? split acc "") < width] [
            acc: rejoin [acc pad]
        ]
        acc
    ]

    ; --- block utils ---

    block-sum: func [blk] [
        either empty? blk [0] [(first blk) + block-sum next blk]
    ]
    block-product: func [blk] [
        either empty? blk [1] [(first blk) * block-product next blk]
    ]
    block-len: func [blk] [length? blk]
    block-mean: func [blk] [
        either empty? blk [0.0] [(block-sum blk) / (length? blk)]
    ]
    ; `mean` is the common numeric alias.
    mean: :block-mean
    reverse-of: func [blk] [
        acc: copy []
        n: length? blk
        i: n
        while [i >= 1] [
            append acc (pick blk i)
            i: i - 1
        ]
        acc
    ]
    ; Flatten one level of nesting: [[1 2] [3]] -> [1 2 3].
    flatten: func [blk] [
        acc: copy []
        foreach v blk [
            either block? v [
                foreach w v [append acc w]
            ] [
                append acc v
            ]
        ]
        acc
    ]
    min-of: func [blk] [
        either empty? blk [none] [
            m: first blk
            n: length? blk
            i: 2
            while [i <= n] [
                v: pick blk i
                if v < m [m: v]
                i: i + 1
            ]
            m
        ]
    ]
    max-of: func [blk] [
        either empty? blk [none] [
            m: first blk
            n: length? blk
            i: 2
            while [i <= n] [
                v: pick blk i
                if v > m [m: v]
                i: i + 1
            ]
            m
        ]
    ]
    ; Insert `sep` between each element: [1 2 3] "/" -> [1 "/" 2 "/" 3]
    intersperse: func [blk sep] [
        either empty? blk [copy []] [
            acc: copy []
            append acc (first blk)
            foreach v (next blk) [
                append acc sep
                append acc v
            ]
            acc
        ]
    ]
    ; Split `blk` into chunks of `n` elements (last chunk may be short).
    chunk: func [blk n] [
        acc: copy []
        cur: copy []
        i: 0
        len: length? blk
        idx: 1
        while [idx <= len] [
            append cur (pick blk idx)
            i: i + 1
            if i = n [
                append acc cur
                cur: copy []
                i: 0
            ]
            idx: idx + 1
        ]
        if i > 0 [append acc cur]
        acc
    ]

    ; --- math utils ---

    gcd: func [a b] [either b = 0 [either a < 0 [0 - a] [a]] [gcd b (a // b)]]
    lcm: func [a b] [either (a * b) = 0 [0] [((either a < 0 [0 - a] [a]) * (either b < 0 [0 - b] [b])) / gcd a b]]
    sign-of: func [n] [either n < 0 [-1] [either n > 0 [1] [0]]]
    clamp: func [n lo hi] [
        either n < lo [lo] [either n > hi [hi] [n]]
    ]
    ; Iterative factorial (avoids deep recursion; named `-iter` to avoid
    ; colliding with user `factorial` defs in examples/func.red etc.).
    factorial-iter: func [n] [
        acc: 1
        i: 2
        while [i <= n] [
            acc: acc * i
            i: i + 1
        ]
        acc
    ]

    ; --- range / sequence ---

    range-of: func [from to] [
        acc: copy []
        i: from
        while [i <= to] [
            append acc i
            i: i + 1
        ]
        acc
    ]

    ; --- sort (pure Red; M30 deferred `sort` as a native) ---

    ; Selection sort. Mutates the input block (matches Red's `sort`
    ; semantics — use `sort copy blk` for a non-mutating sort). Returns
    ; the sorted block.
    sort: func [blk] [
        n: length? blk
        i: 1
        while [i < n] [
            j: i + 1
            min-idx: i
            while [j <= n] [
                if (pick blk j) < (pick blk min-idx) [min-idx: j]
                j: j + 1
            ]
            ; swap i and min-idx
            tmp: pick blk i
            poke blk i (pick blk min-idx)
            poke blk min-idx tmp
            i: i + 1
        ]
        blk
    ]

    export [
        str-upper str-lower starts-with? ends-with? contains? str-join
        repeat-str pad-left pad-right
        block-sum block-product block-len block-mean mean reverse-of
        flatten min-of max-of intersperse chunk
        gcd lcm sign-of clamp factorial-iter
        range-of
        sort
    ]
]
