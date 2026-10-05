; OTP-style supervisor library (Phase 1 stub).
;
; Embedded into the binary via include_str! (stdlib.rs) and cached on
; env.modules['supervisor] by ensure_supervisor_module (stdlib.rs) so
; `import 'supervisor` resolves without a filesystem path.
;
; Phase 2 will fill in:
;   - supervise [child-spec ...]  — spawn supervisor + children, link self
;   - supervisor-send sup 'id msg — route a message to a named child
;   - child spec helpers, one-for-one restart strategy, max-restarts counter
; Built on the existing primitives: spawn-actor, send-actor, link, run-actors,
; and the `self` native (Phase 1).
;
; No `Red []` header: this source is include_str!-loaded via load_source
; (which doesn't strip a header), matching stdlib.red's convention.

module 'supervisor [
    export []
]
