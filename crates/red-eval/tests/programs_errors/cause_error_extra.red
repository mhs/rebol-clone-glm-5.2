Red []
; Exercise cause-error block form (1-arg block) and 4-arg structured form.
; The existing cause_error.red only tests the 1-arg string form.
cause-error [type: 'math message: "boom" code: 42]
