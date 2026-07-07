Red []
; Exercise make error! with where:/by:/near: keyword arms in parse_error_block.
; The existing error_construct.red only tests message:/code:/type:/args:.
probe make error! [message: "m" where: 'foo]
probe make error! [message: "m" by: 'bar]
probe make error! [message: "m" near: [1 2]]
