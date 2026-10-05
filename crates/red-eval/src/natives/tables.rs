//! Shared argument-collection tables for the VM compiler and the walker.
//!
//! Both `vm/compiler.rs::collect_args` and `interp_walker.rs::collect_call_args`
//! implement the same argument-fetching protocol. The per-native quirks
//! below used to be duplicated as inline `matches!` lists in both files;
//! any drift between the copies silently produced VM/walker parity
//! divergences (see KNOWN_ISSUES.md). They now live here, once.

use red_core::value::{Binding, Symbol, Value};

/// Natives whose *first* argument is pushed unevaluated (the literal token,
/// not a compiled/evaluated expression). Both the VM (`collect_args`) and
/// the walker (`collect_call_args`) take the word/name as-is.
///
/// (`import` is NOT in this set — `import m` needs `m` evaluated to the
/// module value, while `import 'name` is a LitWord that evaluates to itself.)
pub(crate) fn is_uneval_first_native(sym: &str) -> bool {
    matches!(
        sym,
        "repeat"
            | "foreach"
            | "forall"
            | "for"
            | "forskip"
            | "map-each"
            | "remove-each"
            | "make"
            | "to"
            | "default"
            | "module"
            | "bound?"
            | "bind?"
            | "context-of"
            | "bind-of"
            | "dump"
    )
}

/// `module` has variable arity: 2 for `module 'name [body]` (the next value
/// is a Word-family — the name), 1 for `module [body]` (the next value is
/// the body Block). Both call sites peek the first arg the same way.
pub(crate) fn module_arity_override(next: Option<&Value>) -> Option<usize> {
    match next {
        Some(
            Value::Word { .. }
            | Value::GetWord { .. }
            | Value::LitWord { .. }
            | Value::SetWord { .. },
        ) => Some(2),
        Some(Value::Block { .. }) => Some(1),
        _ => None,
    }
}

/// `loop count block` (arity 2) vs `loop block` (arity 1, infinite). Peek
/// the first arg: Integer/Float → 2, Block/Paren → 1.
pub(crate) fn loop_arity_override(next: Option<&Value>) -> Option<usize> {
    match next {
        Some(Value::Integer { .. }) | Some(Value::Float { .. }) => Some(2),
        Some(Value::Block { .. }) | Some(Value::Paren { .. }) => Some(1),
        _ => None,
    }
}

/// If `v` is an unbound `Word`/`GetWord`, return its name — the shape both
/// `infix_native_at` (VM + walker) and the variadic-termination predicates
/// (`Compiler::is_native_word_at_dyn` / `is_native_word`) match on. The
/// caller performs the registry lookup (`natives.get(sym)` / `contains`).
pub(crate) fn unbound_word_sym(v: &Value) -> Option<&Symbol> {
    let sym = match v {
        Value::Word { sym, binding, .. } | Value::GetWord { sym, binding, .. } => {
            if !matches!(binding, Binding::Unbound) {
                return None;
            }
            sym
        }
        _ => return None,
    };
    Some(sym)
}
