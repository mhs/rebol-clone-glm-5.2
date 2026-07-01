; stringutils — a module of string manipulation utilities.
; Demonstrates: a module with string-processing functions, some using
; the stdlib (str-upper, str-join) and some standalone.

module 'stringutils [
    ; Capitalize each word in a space-separated string.
    capwords: func [s] [
        words: split s " "
        result: copy []
        foreach w words [
            ; Capitalize first char + rest as-is (simplified: uppercase whole word).
            append result (str-upper w)
        ]
        str-join result " "
    ]
    ; Convert snake_case to camelCase.
    snake-to-camel: func [s] [
        parts: split s "-"
        either (length? parts) <= 1 [(first parts)] [
            acc: copy (first parts)
            foreach w (next parts) [
                chars: split w ""
                either empty? chars [acc] [
                    head-char: str-upper (first chars)
                    rest: rejoin (next chars)
                    acc: rejoin [acc head-char rest]
                ]
            ]
            acc
        ]
    ]
    ; Truncate `s` to `n` chars, appending "..." if truncated.
    truncate-to: func [s n] [
        chars: split s ""
        either (length? chars) <= n [s] [
            rejoin [(rejoin (copy/part chars n)) "..."]
        ]
    ]

    export [capwords snake-to-camel truncate-to]
]
