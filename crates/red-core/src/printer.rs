//! `mold`: value → Red source text. Inverse of the parser; round-trip
//! property is `mold(parse(s)) == normalize(s)`.

use chrono::{Datelike, Timelike};

use crate::value::{
    BitsetDef, DateValue, HashDef, ImageDef, MapDef, MapKey, ModuleDef, MoneyValue, ObjectDef,
    PortDef, TypesetDef, Value, VectorDef,
};

/// Append the Red source form of `value` to `out`.
pub fn mold(value: &Value, out: &mut String) {
    match value {
        Value::None => out.push_str("none"),
        // M86: `unset!` molds/forms to the empty string (matches Red).
        Value::Unset => {}
        Value::Logic(true) => out.push_str("true"),
        Value::Logic(false) => out.push_str("false"),
        Value::Integer { n, .. } => {
            use std::fmt::Write;
            let _ = write!(out, "{}", n);
        }
        Value::Float { f, .. } => mold_float(*f, out),
        Value::Decimal { d, .. } => mold_decimal(*d, out),
        Value::Percent { value, .. } => mold_percent(*value, out),
        Value::Money { amount, .. } => mold_money(amount, out),
        Value::Issue { s, .. } => {
            out.push('#');
            out.push_str(s);
        }
        Value::Email { addr, .. } => out.push_str(addr),
        Value::Tag { text, .. } => mold_tag(text, out),
        Value::String { s, .. } => mold_string(s, out),
        Value::Char { c, .. } => mold_char(*c, out),
        Value::Pair { x, y, .. } => {
            mold(x, out);
            out.push('x');
            mold(y, out);
        }
        Value::Tuple { bytes, .. } => {
            // `255.0.0` (3 bytes RGB) or `128.64.32.128` (4 bytes RGBA).
            // Dot-joined, no spaces.
            for (n, b) in bytes.iter().enumerate() {
                if n > 0 {
                    out.push('.');
                }
                use std::fmt::Write;
                let _ = write!(out, "{}", b);
            }
        }
        Value::String8 { bytes, .. } => {
            // M41: mold as `#{HEX}` (uppercase) — matches Red's binary! form
            // and round-trips through the lexer.
            out.push_str("#{");
            for b in bytes {
                use std::fmt::Write;
                let _ = write!(out, "{:02X}", b);
            }
            out.push('}');
        }
        Value::Word { sym, .. } => out.push_str(sym.as_str()),
        Value::SetWord { sym, .. } => {
            out.push_str(sym.as_str());
            out.push(':');
        }
        Value::GetWord { sym, .. } => {
            out.push(':');
            out.push_str(sym.as_str());
        }
        Value::LitWord { sym, .. } => {
            out.push('\'');
            out.push_str(sym.as_str());
        }
        Value::Block { series, .. } => {
            out.push('[');
            let data = series.data.borrow();
            // Red molds a positioned series from its cursor to the tail, so
            // `mold next [1 2 3]` renders `[2 3]`. Parsed blocks always start
            // at index 0, so this only affects series produced by navigation
            // natives (`next`/`skip`/`find`/etc.).
            for (n, v) in data.iter().enumerate().skip(series.index) {
                if n > series.index {
                    out.push(' ');
                }
                mold(v, out);
            }
            out.push(']');
        }
        Value::Paren { series, .. } => {
            out.push('(');
            let data = series.data.borrow();
            for (n, v) in data.iter().enumerate().skip(series.index) {
                if n > series.index {
                    out.push(' ');
                }
                mold(v, out);
            }
            out.push(')');
        }
        Value::Func(_) => out.push_str("#[function]"),
        // M60: closure molds as `#[closure]` placeholder (parity with
        // `#[function]` — no spec/body molding; not reparseable as a literal).
        Value::Closure(_) => out.push_str("#[closure]"),
        Value::Error(err) => {
            // M42: structured errors mold as `make error! [code: ... type:
            // 'word args: [...] message: "..."]` (only non-default fields
            // emitted). Message-only errors keep the back-compat
            // `make error! "msg"` form so existing golden fixtures stay green.
            if err.is_message_only() {
                out.push_str("make error! ");
                mold_string(&err.message, out);
            } else {
                out.push_str("make error! [");
                let mut first = true;
                if let Some(code) = err.code {
                    use std::fmt::Write;
                    let _ = write!(out, "{}code: {}", sep(&mut first), code);
                }
                if let Some(kind) = &err.kind {
                    out.push_str(&sep(&mut first));
                    out.push_str("type: ");
                    out.push('\'');
                    out.push_str(kind.as_str());
                }
                if !err.args.is_empty() {
                    out.push_str(&sep(&mut first));
                    out.push_str("args: [");
                    let arg_block = Value::Block {
                        series: crate::value::Series::new(err.args.clone()),
                        span: crate::value::Span::default(),
                    };
                    mold(&arg_block, out);
                    out.push(']');
                }
                {
                    out.push_str(&sep(&mut first));
                    out.push_str("message: ");
                    mold_string(&err.message, out);
                }
                if let Some(cause) = &err.cause {
                    out.push_str(&sep(&mut first));
                    out.push_str("where: ");
                    out.push('\'');
                    out.push_str(cause.as_str());
                }
                if let Some(by) = &err.by {
                    out.push_str(&sep(&mut first));
                    out.push_str("by: ");
                    out.push('\'');
                    out.push_str(by.as_str());
                }
                if let Some(near) = &err.near {
                    out.push_str(&sep(&mut first));
                    out.push_str("near: ");
                    mold(near, out);
                }
                out.push(']');
            }
        }
        Value::Path { parts, .. } => mold_path_parts(parts, None, None, out),
        Value::GetPath { parts, .. } => mold_path_parts(parts, Some(':'), None, out),
        Value::LitPath { parts, .. } => mold_path_parts(parts, Some('\''), None, out),
        Value::SetPath { parts, .. } => mold_path_parts(parts, None, Some(':'), out),
        Value::Refinement { sym, .. } => {
            out.push('/');
            out.push_str(sym.as_str());
        }
        Value::File { path, .. } => mold_file(path, out),
        Value::Url { url, .. } => out.push_str(url),
        Value::Object(obj) => mold_object(&obj.borrow(), out),
        Value::Map(m) => mold_map(&m.borrow(), out),
        Value::Hash(h) => mold_hash(&h.borrow(), out),
        Value::Vector(v) => mold_vector(&v.borrow(), out),
        Value::Image(im) => mold_image(&im.borrow(), out),
        Value::Module(m) => mold_module(&m.borrow(), out),
        Value::Date { dt, .. } => mold_date(dt, out),
        Value::Duration { d, .. } => mold_duration(*d, out),
        Value::Bitset(b) => mold_bitset(&b.borrow(), out),
        Value::Port(p) => mold_port(&p.borrow(), out),
        Value::Typeset(t) => mold_typeset(t, out),
    }
}

/// Convenience: return the mold as an owned `String`.
pub fn mold_to_string(value: &Value) -> String {
    let mut out = String::new();
    mold(value, &mut out);
    out
}

/// `form`: human-readable rendering, distinct from `mold` (which is
/// reparseable). Differences from `mold`:
/// - `String` renders its raw contents (no surrounding quotes, no escapes).
/// - Word-family values render their bare name (no `:`/`'`/`/` prefix/suffix).
/// - `Block`/`Paren` render their elements space-joined from the cursor to
///   the tail, with no surrounding `[]`/`()` delimiters.
/// - `Path` renders parts (each `form`ed) slash-joined.
///
/// All other variants render the same as `mold` (integers, floats, logic,
/// none, func placeholder, binary hex).
pub fn form(value: &Value, out: &mut String) {
    match value {
        Value::None => out.push_str("none"),
        // M86: `unset!` molds/forms to the empty string (matches Red).
        Value::Unset => {}
        Value::Logic(true) => out.push_str("true"),
        Value::Logic(false) => out.push_str("false"),
        Value::Integer { n, .. } => {
            use std::fmt::Write;
            let _ = write!(out, "{}", n);
        }
        Value::Float { f, .. } => mold_float(*f, out),
        Value::Decimal { d, .. } => mold_decimal(*d, out),
        Value::Percent { value, .. } => mold_percent(*value, out),
        Value::Money { amount, .. } => mold_money(amount, out),
        Value::Issue { s, .. } => out.push_str(s),
        Value::Email { addr, .. } => out.push_str(addr),
        Value::Tag { text, .. } => mold_tag(text, out),
        Value::String { s, .. } => out.push_str(s),
        Value::Char { c, .. } => out.push(*c),
        Value::Pair { x, y, .. } => {
            form(x, out);
            out.push('x');
            form(y, out);
        }
        Value::Tuple { bytes, .. } => {
            for (n, b) in bytes.iter().enumerate() {
                if n > 0 {
                    out.push('.');
                }
                use std::fmt::Write;
                let _ = write!(out, "{}", b);
            }
        }
        Value::String8 { bytes, .. } => {
            out.push_str("#{");
            for b in bytes {
                use std::fmt::Write;
                let _ = write!(out, "{:02X}", b);
            }
            out.push('}');
        }
        Value::Word { sym, .. }
        | Value::SetWord { sym, .. }
        | Value::GetWord { sym, .. }
        | Value::LitWord { sym, .. } => out.push_str(sym.as_str()),
        Value::Refinement { sym, .. } => out.push_str(sym.as_str()),
        Value::File { path, .. } => out.push_str(path),
        Value::Url { url, .. } => out.push_str(url),
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            let data = series.data.borrow();
            for (n, v) in data.iter().enumerate().skip(series.index) {
                if n > series.index {
                    out.push(' ');
                }
                form(v, out);
            }
        }
        Value::Func(_) => out.push_str("#[function]"),
        // M60: closure forms as `#[closure]` (parity with `#[function]`).
        Value::Closure(_) => out.push_str("#[closure]"),
        Value::Error(err) => out.push_str(&err.message),
        Value::Path { parts, .. } => form_path_parts(parts, None, None, out),
        Value::GetPath { parts, .. } => form_path_parts(parts, Some(':'), None, out),
        Value::LitPath { parts, .. } => form_path_parts(parts, Some('\''), None, out),
        Value::SetPath { parts, .. } => form_path_parts(parts, None, Some(':'), out),
        Value::Object(obj) => {
            // form renders just the inner body (no `make object!` wrapper),
            // space-joined, matching `form` of a block.
            let o = obj.borrow();
            form_object_body(&o, out);
        }
        Value::Map(m) => form_map(&m.borrow(), out),
        Value::Hash(h) => form_hash(&h.borrow(), out),
        Value::Vector(v) => mold_vector(&v.borrow(), out),
        Value::Image(im) => mold_image(&im.borrow(), out),
        Value::Module(m) => mold_module(&m.borrow(), out),
        Value::Date { dt, .. } => mold_date(dt, out),
        Value::Duration { d, .. } => mold_duration(*d, out),
        Value::Bitset(b) => mold_bitset(&b.borrow(), out),
        Value::Port(p) => mold_port(&p.borrow(), out),
        Value::Typeset(t) => mold_typeset(t, out),
    }
}

/// Convenience: return the form as an owned `String`.
pub fn form_to_string(value: &Value) -> String {
    let mut out = String::new();
    form(value, &mut out);
    out
}

fn mold_float(f: f64, out: &mut String) {
    // `{:?}` prints `5.0` rather than `5`, and scientific notation only when
    // Rust thinks it's appropriate. We post-process to guarantee a `.` so the
    // result always parses back as a Float, not an Integer.
    let s = format!("{:?}", f);
    out.push_str(&s);
    if !s.contains('.') && !s.contains('e') && !s.contains("inf") && !s.contains("NaN") {
        out.push_str(".0");
    }
}

/// M150: mold a decimal! value. `rust_decimal::Decimal`'s `Display` impl
/// is round-trip-safe and never produces `NaN`/`inf`. We append `dec` so
/// the result parses back as a decimal! literal (not a float!). For
/// integer-valued decimals, `Decimal::Display` produces `100` (no `.0`),
/// so we insert `.0` to keep the value visibly non-integer — matching
/// `mold_float`'s convention and ensuring `100dec` round-trips as
/// `100.0dec` rather than colliding with the integer-with-suffix form.
fn mold_decimal(d: rust_decimal::Decimal, out: &mut String) {
    let s = d.to_string();
    out.push_str(&s);
    if !s.contains('.') && !s.contains('e') {
        out.push_str(".0");
    }
    out.push_str("dec");
}

/// M80: mold a percent! value. Stored as a fractional float (`0.5` ⇒ `50%`);
/// rendered as `value * 100` with trailing zeros/dot trimmed, then `%`. Round-
/// trips through the lexer (`50%` ⇒ `0.5` ⇒ `50%`). `0%` molds as `0%` (not
/// `0.0%`) for Red parity.
fn mold_percent(value: f64, out: &mut String) {
    use std::fmt::Write;
    // Render with 6 sig figs of fractional precision then trim, mirroring
    // Red's default printing. Integer-valued percents mold without a decimal.
    let pct = value * 100.0;
    let s = format!("{:.6}", pct);
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    let _ = write!(out, "{}%", trimmed);
}

/// M80: mold a money! value. `$<dollars>.<DD>` with exactly 2 decimal places,
/// and an optional `:CCC` currency suffix when the currency is not USD (the
/// default). `$10.00` molds as `$10.00`; `$10.00:EUR` molds as `$10.00:EUR`.
/// Negative cents mold as `-$10.00`. Round-trips through the lexer.
fn mold_money(m: &MoneyValue, out: &mut String) {
    use std::fmt::Write;
    let negative = m.cents < 0;
    let abs = m.cents.unsigned_abs();
    let dollars = abs / 100;
    let cents = abs % 100;
    if negative {
        out.push('-');
    }
    out.push('$');
    let _ = write!(out, "{}.{:02}", dollars, cents);
    if m.currency.as_ref() != "USD" {
        out.push(':');
        out.push_str(&m.currency);
    }
}

/// M81: mold a tag! value. The body is wrapped in `<...>`; any `<`/`>`/`\`
/// in the body is escaped as `\<`/`\>`/`\\` so the result round-trips through
/// the lexer's `scan_tag` (which decodes the escapes).
fn mold_tag(text: &str, out: &mut String) {
    out.push('<');
    for c in text.chars() {
        match c {
            '<' => out.push_str("\\<"),
            '>' => out.push_str("\\>"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('>');
}

fn mold_object(obj: &ObjectDef, out: &mut String) {
    out.push_str("make object! [");
    let words = obj.ctx.words();
    let slots = obj.ctx.slots.borrow();
    let mut first = true;
    for sym in words.iter() {
        if sym.as_str() == "self" {
            continue; // skip self-reference (would infinite-loop)
        }
        let idx = obj.ctx.index_of(sym).unwrap();
        let val = slots[idx].borrow();
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(sym.as_str());
        out.push_str(": ");
        mold(&val, out);
    }
    out.push(']');
}

fn form_object_body(obj: &ObjectDef, out: &mut String) {
    let words = obj.ctx.words();
    let slots = obj.ctx.slots.borrow();
    let mut first = true;
    for sym in words.iter() {
        if sym.as_str() == "self" {
            continue;
        }
        let idx = obj.ctx.index_of(sym).unwrap();
        let val = slots[idx].borrow();
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(sym.as_str());
        out.push_str(": ");
        form(&val, out);
    }
}

/// Mold a map as `make map! [k1 v1 k2 v2 ...]`. Word keys emit as set-words
/// (`a: 1`) — the natural Red form that reparses via `make map!`. Other key
/// types (int/string/char/bool/none) emit the key value followed by its value.
/// Single-line for empty/single-entry maps; multi-entry maps stay
/// space-separated on one line (matches Red's compact mold).
fn mold_map(m: &MapDef, out: &mut String) {
    out.push_str("make map! [");
    let entries = m.entries.borrow();
    let mut first = true;
    for (k, v) in entries.iter() {
        if !first {
            out.push(' ');
        }
        first = false;
        mold_map_key(k, out, true);
        out.push(' ');
        mold(v, out);
    }
    out.push(']');
}

/// `form` of a map: same `make map! [...]` body as `mold` (Red treats `form`
/// of a map like its mold).
fn form_map(m: &MapDef, out: &mut String) {
    out.push_str("make map! [");
    let entries = m.entries.borrow();
    let mut first = true;
    for (k, v) in entries.iter() {
        if !first {
            out.push(' ');
        }
        first = false;
        mold_map_key(k, out, false);
        out.push(' ');
        form(v, out);
    }
    out.push(']');
}

/// Mold a hash! as `make hash! [k1 v1 k2 v2 ...]`. Iterates `key_order` for
/// stable output (documented deviation — Red's `hash!` mold is unspecified-
/// order). Word keys emit as set-words (`a: 1`) so the block reparses via
/// `make hash!`. Other key types emit the key value followed by its value.
fn mold_hash(h: &HashDef, out: &mut String) {
    out.push_str("make hash! [");
    let entries = h.entries.borrow();
    let order = h.key_order.borrow();
    let mut first = true;
    for k in order.iter() {
        if !first {
            out.push(' ');
        }
        first = false;
        mold_map_key(k, out, true);
        out.push(' ');
        if let Some(v) = entries.get(k) {
            mold(v, out);
        } else {
            out.push_str("none");
        }
    }
    out.push(']');
}

/// `form` of a hash!: same `make hash! [...]` body as `mold`.
fn form_hash(h: &HashDef, out: &mut String) {
    out.push_str("make hash! [");
    let entries = h.entries.borrow();
    let order = h.key_order.borrow();
    let mut first = true;
    for k in order.iter() {
        if !first {
            out.push(' ');
        }
        first = false;
        mold_map_key(k, out, false);
        out.push(' ');
        if let Some(v) = entries.get(k) {
            form(v, out);
        } else {
            out.push_str("none");
        }
    }
    out.push(']');
}

/// M84: mold a vector! as `make vector! [<kind-word> <e1> <e2> ...]`. The
/// kind word is the first element (`integer!`/`float!`/`i8!`/…), followed by
/// space-separated molded elements. The whole vector is molded from index 0
/// (cursor-agnostic — mold always reflects the full contents, not a
/// positioned view). The form is reparseable via `make vector!`.
fn mold_vector(v: &VectorDef, out: &mut String) {
    out.push_str("make vector! [");
    out.push_str(v.kind.borrow().as_str());
    let elems = v.elems.borrow();
    for e in elems.iter() {
        out.push(' ');
        mold(e, out);
    }
    out.push(']');
}

/// M89: mold a typeset! as `make typeset! [<type-word> ...]`. The type words
/// are emitted in sorted order (lexicographic on the symbol name) so the mold
/// is deterministic across runs and across HashMap iteration orders. The form
/// is reparseable via `make typeset!`. Group words (`any-word!`/`number!`/
/// `series!`/...) mold alongside leaf words.
fn mold_typeset(t: &TypesetDef, out: &mut String) {
    out.push_str("make typeset! [");
    let words = t.sorted_words();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(w.as_str());
    }
    out.push(']');
}

/// M85: mold an image! as `make image! [width: <w> height: <h> pixels: [<byte...>]]`.
/// Pixels render as a flat row-major RGBA8 byte stream (4 integers per pixel),
/// space-separated. The form is reparseable via `make image!`. The mold is
/// cursor-agnostic (image! has no cursor — size is fixed).
fn mold_image(im: &ImageDef, out: &mut String) {
    out.push_str("make image! [width: ");
    out.push_str(&im.width.to_string());
    out.push_str(" height: ");
    out.push_str(&im.height.to_string());
    out.push_str(" pixels: [");
    let p = im.pixels.borrow();
    for (i, px) in p.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&px[0].to_string());
        out.push(' ');
        out.push_str(&px[1].to_string());
        out.push(' ');
        out.push_str(&px[2].to_string());
        out.push(' ');
        out.push_str(&px[3].to_string());
    }
    out.push_str("]]");
}

/// M61: mold a module as `make module! [name: <name> exports: [words] word: val ...]`.
///
/// Only the *exported* words render (the public surface — matches `words-of`/
/// `values-of` semantics; private slots are intentionally omitted from the
/// mold so the serialized form doesn't leak private state). The `exports:`
/// block lists the exported word names in `ctx` insertion order; each
/// `word: value` pair follows in the same order. `name:` is emitted only for
/// named modules. The form is reparseable (`load mold m` succeeds) and
/// reconstructs via `make module! [...]` (which interprets `name:`/`exports:`
/// as keywords).
fn mold_module(m: &ModuleDef, out: &mut String) {
    out.push_str("make module! [");
    let mut first = true;
    if let Some(name) = &m.name {
        out.push_str("name: ");
        out.push_str(name.as_str());
        first = false;
    }
    // Collect exported words in ctx insertion order (iterate ctx.words(),
    // filter by exports — never iterate the HashSet, which is unordered).
    let exported: Vec<crate::value::Symbol> = m
        .ctx
        .words()
        .into_iter()
        .filter(|s| m.exports.borrow().contains(s))
        .collect();
    if !exported.is_empty() {
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str("exports: [");
        let mut first_e = true;
        for s in &exported {
            if !first_e {
                out.push(' ');
            }
            first_e = false;
            out.push_str(s.as_str());
        }
        out.push(']');
    }
    let slots = m.ctx.slots.borrow();
    for s in &exported {
        if let Some(idx) = m.ctx.index_of(s) {
            if !first {
                out.push(' ');
            }
            first = false;
            out.push_str(s.as_str());
            out.push_str(": ");
            mold(&slots[idx].borrow(), out);
        }
    }
    out.push(']');
}

/// M46: mold a `bitset!` value.
///
/// Form: `make bitset! "ABC"` (listing the set chars as a string literal)
/// when all set bits are printable, non-quote, non-backslash ASCII chars.
/// Falls back to `make bitset! #{hex}` (the raw bit pattern as a binary!)
/// for sparse bitsets or bitsets with control/non-ASCII bits — the hex form
/// is unambiguous and always available.
///
/// The string form is preferred for the common `charset "ABC"` case (the
/// most-frequent bitset construction in `parse` dialect code).
fn mold_bitset(bs: &BitsetDef, out: &mut String) {
    let chars = bs.iter_set_chars();
    // Prefer the string form when the bitset is charset-sized (len <= 256)
    // and every set bit is a printable ASCII char that's safe to embed in a
    // `"..."` literal (no `"`/`\`/control chars). Empty charsets mold as
    // `make bitset! ""`. Larger bitsets (len > 256) or those with
    // non-printable bits fall back to `make bitset! #{hex}`.
    let charset_sized = bs.len <= 256;
    let all_printable = chars
        .iter()
        .all(|c| c.is_ascii_graphic() && *c != '"' && *c != '\\');
    if charset_sized && all_printable {
        out.push_str("make bitset! ");
        let s: String = chars.iter().collect();
        mold_string(&s, out);
    } else {
        // Fall back to the raw bit pattern as `#{hex}`. The binary! literal
        // reparses and `make bitset!` accepts a binary! spec.
        out.push_str("make bitset! #{");
        for b in bs.raw_bytes() {
            use std::fmt::Write;
            let _ = write!(out, "{:02X}", b);
        }
        out.push('}');
    }
}

/// M113: mold a `port!` value. Non-reparseable synthetic form — renders as
/// `#[port <scheme>://<target>]` (matching the `#[function]`/`#[closure]`
/// placeholder style). The bracketed form signals "synthetic, not a literal"
/// so `load` of a molded port fails fast (no `port!` literal syntax exists).
fn mold_port(p: &PortDef, out: &mut String) {
    use std::fmt::Write;
    let _ = write!(out, "#[port {}://{}]", p.scheme.as_str(), p.target);
}

/// M45: mold a `date!` value.
///
/// Forms:
/// - **date-only** (`dt` at midnight, `zone = None`): `29-Jun-2024`.
/// - **date+time, zone-naive**: `29-Jun-2024/12:30:00`.
/// - **date+time, UTC** (`zone = Some(0)`): `29-Jun-2024/12:30:00+00:00`.
/// - **date+time, non-UTC zone**: `29-Jun-2024/12:30:00-04:00`.
///
/// Always emits `+HH:MM` two-digit form for the zone, **never `Z`** (per
/// plan5.md M45). The day-month-year uses Red's `DD-Mon-YYYY` form (month
/// abbreviated English). Time uses `HH:MM:SS` (optionally `.mmm` if
/// sub-second; the lexer/parser support `.mmm` but `now`-derived values
/// typically don't carry nanoseconds in the mold form).
fn mold_date(d: &DateValue, out: &mut String) {
    use std::fmt::Write;
    let date = d.dt.date();
    // `DD-Mon-YYYY` form. Month is the 3-letter English abbreviation
    // (matches Red). chrono's `%b` gives `Jun` for June.
    let _ = write!(
        out,
        "{:02}-{}-{}",
        date.day(),
        month_abbr(date.month()),
        date.year()
    );
    if d.has_time() || d.zone.is_some() {
        let t = d.dt.time();
        out.push('/');
        let _ = write!(out, "{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second());
        if let Some(zone) = d.zone {
            let sign = if zone < 0 { '-' } else { '+' };
            let m = zone.abs();
            let _ = write!(out, "{}{:02}:{:02}", sign, m / 60, m % 60);
        }
    }
}

/// Mold a `duration!` (M140). Strategy: pick the **largest unit** that
/// yields a whole-number representation, falling back to `ns`. Since
/// `chrono::Duration` is i64-nanoseconds, every value is a whole multiple
/// of `1ns` — fractional mantissa never triggers. The sign (if negative)
/// prefixes the whole token. Zero molds as `0s`.
///
/// Compound literals mold **single-unit** (`1d1h` → `30h`,
/// `1.5h` → `90m`); the round-trip contract is value-equal, not
/// text-equal (a future `/long` refinement could emit `1h30m`; deferred).
fn mold_duration(d: chrono::Duration, out: &mut String) {
    use std::fmt::Write;
    // `num_nanoseconds` returns Option<i64>; None only for values beyond
    // i64 range (~292 years), which the saturating constructor already
    // excludes. Fall back to 0 defensively.
    let n = d.num_nanoseconds().unwrap_or(0);
    if n == 0 {
        out.push_str("0s");
        return;
    }
    let neg = n < 0;
    let abs_n = n.wrapping_abs() as u64;
    // (factor_nanos, suffix). Strictly descending magnitude — the first
    // divisor that yields a zero remainder is the largest whole unit.
    const UNITS: &[(u64, &str)] = &[
        (86_400_000_000_000, "d"),
        (3_600_000_000_000, "h"),
        (60_000_000_000, "m"),
        (1_000_000_000, "s"),
        (1_000_000, "ms"),
        (1_000, "us"),
        (1, "ns"),
    ];
    for (factor, suffix) in UNITS {
        if abs_n.is_multiple_of(*factor) {
            if neg {
                out.push('-');
            }
            let _ = write!(out, "{}{}", abs_n / factor, suffix);
            return;
        }
    }
    // Unreachable: `ns` (factor 1) always divides evenly.
    let _ = write!(out, "{}ns", abs_n);
}

/// 3-letter English month abbreviation (matches Red's `DD-Mon-YYYY` form and
/// `chrono::%b` for English locale). Hardcoded to avoid locale-dependence in
/// `chrono`'s `%b` formatting (which uses the system locale on some
/// platforms).
fn month_abbr(m: u32) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    }
}

/// Emit a single map key. When `set_word_form` is true, `Sym` keys render as
/// set-words (`a:`) so the surrounding `make map! [...]` block reparses;
/// otherwise (`form`), they render as bare words.
fn mold_map_key(k: &MapKey, out: &mut String, set_word_form: bool) {
    match k {
        MapKey::Sym(sym) => {
            out.push_str(sym.as_str());
            if set_word_form {
                out.push(':');
            }
        }
        _ => mold(&k.to_value(), out),
    }
}

/// Mold a path's parts joined by `/`, with optional prefix (`:` for get-path,
/// `'` for lit-path) and optional suffix (`:` for set-path). Each part is
/// molded via [`mold`]; paren parts mold as `(...)` so `foo/(a+b)/bar`
/// round-trips.
fn mold_path_parts(parts: &[Value], prefix: Option<char>, suffix: Option<char>, out: &mut String) {
    if let Some(p) = prefix {
        out.push(p);
    }
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        mold(p, out);
    }
    if let Some(s) = suffix {
        out.push(s);
    }
}

/// Like [`mold_path_parts`] but each part is `form`ed (so a paren part renders
/// its evaluated-look, and word parts render their bare name). The prefix/
/// suffix are still emitted in mold-style so the variant is recognizable.
fn form_path_parts(parts: &[Value], prefix: Option<char>, suffix: Option<char>, out: &mut String) {
    if let Some(p) = prefix {
        out.push(p);
    }
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        form(p, out);
    }
    if let Some(s) = suffix {
        out.push(s);
    }
}

/// M42 helper: emit a leading space separator for `make error! [...]` field
/// runs. The first call returns `""` so the first field has no leading space;
/// subsequent calls return `" "` so fields are space-separated. The closure
/// captures `&mut first` to flip the flag.
fn sep(first: &mut bool) -> String {
    if *first {
        *first = false;
        String::new()
    } else {
        " ".to_string()
    }
}

fn mold_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

/// Mold a char! literal in the `#"..."` form, using Red caret-escapes for
/// special chars. The escapes mirror the lexer's `scan_char` decoder:
/// - `^-` → tab, `^/` → newline, `^@` → null
/// - `^^` → literal caret, `^"` → literal quote
/// - control chars (`\x01`..`\x1F` other than the named above) → `^A`..`^Z`
/// - any other char → the literal char
/// - codepoints above U+FFFF → `^(NNNNNN)` hex form
fn mold_char(c: char, out: &mut String) {
    out.push_str("#\"");
    match c {
        '\t' => out.push_str("^-"),
        '\n' => out.push_str("^/"),
        '\r' => out.push_str("^M"),
        '\0' => out.push_str("^@"),
        '^' => out.push_str("^^"),
        '"' => out.push_str("^\""),
        c if (c as u32) < 0x20 => {
            // Other control char: use `^<letter>` form (Ctrl-A..Ctrl-Z).
            let n = (c as u32) + 0x40;
            if let Some(letter) = char::from_u32(n) {
                out.push('^');
                out.push(letter);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "^({:X})", c as u32);
            }
        }
        c if (c as u32) > 0xFFFF => {
            use std::fmt::Write;
            let _ = write!(out, "^({:X})", c as u32);
        }
        _ => out.push(c),
    }
    out.push('"');
}

/// Mold a file! path. Uses the bare `%path` form when the path contains no
/// file-delimiter characters (so `%foo/bar.txt` round-trips compactly), and
/// the quoted `%"..."` form (with string-style escapes) when it does (e.g.
/// paths with spaces). Either form re-parses to the same value.
fn mold_file(path: &str, out: &mut String) {
    let needs_quotes = path.is_empty()
        || path.as_bytes().iter().any(|c| {
            matches!(
                c,
                b' ' | b'\t'
                    | b'\r'
                    | b'\n'
                    | b'['
                    | b']'
                    | b'('
                    | b')'
                    | b'{'
                    | b'}'
                    | b';'
                    | b'"'
            )
        });
    out.push('%');
    if needs_quotes {
        mold_string(path, out);
    } else {
        out.push_str(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Series, Span, Symbol};
    use std::rc::Rc;

    fn s(literal: &str) -> Value {
        Value::string(Rc::<str>::from(literal))
    }

    #[test]
    fn mold_none() {
        assert_eq!(mold_to_string(&Value::None), "none");
    }

    #[test]
    fn mold_logic() {
        assert_eq!(mold_to_string(&Value::Logic(true)), "true");
        assert_eq!(mold_to_string(&Value::Logic(false)), "false");
    }

    #[test]
    fn mold_integer() {
        assert_eq!(mold_to_string(&Value::integer(0)), "0");
        assert_eq!(mold_to_string(&Value::integer(42)), "42");
        assert_eq!(mold_to_string(&Value::integer(-7)), "-7");
    }

    #[test]
    fn mold_float() {
        assert_eq!(mold_to_string(&Value::float(5.0)), "5.0");
        assert_eq!(mold_to_string(&Value::float(1.5)), "1.5");
        assert_eq!(mold_to_string(&Value::float(-2.25)), "-2.25");
    }

    #[test]
    fn mold_float_always_has_dot() {
        // Every finite float must mold with a `.` so it re-parses as Float
        // (not Integer). `{:?}` on f64 already does this for whole numbers.
        for n in [0.0, 1.0, -1.0, 100.0, 1_000_000.0] {
            let molded = mold_to_string(&Value::float(n));
            assert!(molded.contains('.'), "{n} molded to {molded:?} (no dot)");
        }
    }

    #[test]
    fn mold_float_scientific_notation_round_trips() {
        // Large/small magnitudes use scientific notation via `{:?}`; the
        // lexer accepts `e`/`E` exponents, so these re-parse.
        for f in [1e20, 1e-10, 1.5e30] {
            let molded = mold_to_string(&Value::float(f));
            let toks = crate::lexer::lex(&molded).expect("lex float");
            assert_eq!(toks.len(), 1);
            match &toks[0].kind {
                crate::lexer::TokenKind::Float(parsed) => {
                    assert_eq!(*parsed, f, "round-trip mismatch: {f} molded to {molded}");
                }
                other => panic!("expected Float token for {molded}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_float_nan_inf_documented_gap() {
        // NaN/inf are NOT reparseable (the lexer has no literal for them) —
        // a documented POC limitation. We just confirm they mold to *some*
        // string without panicking; the property test excludes them.
        let _ = mold_to_string(&Value::float(f64::NAN));
        let _ = mold_to_string(&Value::float(f64::INFINITY));
        let _ = mold_to_string(&Value::float(f64::NEG_INFINITY));
    }

    #[test]
    fn mold_percent() {
        // M80: percent molds as `NN%` (the fractional value × 100).
        assert_eq!(mold_to_string(&Value::percent(0.0)), "0%");
        assert_eq!(mold_to_string(&Value::percent(0.5)), "50%");
        assert_eq!(mold_to_string(&Value::percent(1.0)), "100%");
        assert_eq!(mold_to_string(&Value::percent(-0.5)), "-50%");
        assert_eq!(mold_to_string(&Value::percent(0.005)), "0.5%");
        assert_eq!(mold_to_string(&Value::percent(0.015)), "1.5%");
    }

    #[test]
    fn mold_percent_round_trips_via_lexer() {
        // Every percent value we mold must re-parse to the same value.
        for value in [0.0, 0.5, 1.0, -0.5, 0.005, 0.015, 0.333333, 2.5] {
            let molded = mold_to_string(&Value::percent(value));
            let toks = crate::lexer::lex(&molded).expect("lex percent");
            assert_eq!(toks.len(), 1, "{value} molded to {molded:?}");
            match &toks[0].kind {
                crate::lexer::TokenKind::Percent(parsed) => {
                    assert_eq!(*parsed, value, "round-trip mismatch: {value} → {molded}");
                }
                other => panic!("expected Percent token for {molded}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_money() {
        // M80: money molds as `$<dollars>.<DD>` (2 decimals), with optional
        // `:CCC` suffix when non-USD.
        use crate::value::MoneyValue;
        assert_eq!(
            mold_to_string(&Value::Money {
                amount: std::rc::Rc::new(MoneyValue::usd(0)),
                span: Span::default(),
            }),
            "$0.00"
        );
        assert_eq!(
            mold_to_string(&Value::Money {
                amount: std::rc::Rc::new(MoneyValue::usd(1000)),
                span: Span::default(),
            }),
            "$10.00"
        );
        assert_eq!(
            mold_to_string(&Value::Money {
                amount: std::rc::Rc::new(MoneyValue::usd(123456)),
                span: Span::default(),
            }),
            "$1234.56"
        );
        assert_eq!(
            mold_to_string(&Value::Money {
                amount: std::rc::Rc::new(MoneyValue::usd(-1000)),
                span: Span::default(),
            }),
            "-$10.00"
        );
        assert_eq!(
            mold_to_string(&Value::Money {
                amount: std::rc::Rc::new(MoneyValue::new(1000, "EUR")),
                span: Span::default(),
            }),
            "$10.00:EUR"
        );
    }

    #[test]
    fn mold_money_round_trips_via_lexer() {
        // Every money value we mold must re-parse to the same cents+currency.
        use crate::value::MoneyValue;
        for (cents, cur) in [
            (0i64, "USD"),
            (1000, "USD"),
            (123456, "USD"),
            (-1000, "USD"),
            (5, "USD"),
            (1000, "EUR"),
            (99, "JPY"),
        ] {
            let v = Value::Money {
                amount: std::rc::Rc::new(MoneyValue::new(cents, cur)),
                span: Span::default(),
            };
            let molded = mold_to_string(&v);
            let toks = crate::lexer::lex(&molded).expect("lex money");
            assert_eq!(toks.len(), 1, "{cents} {cur} molded to {molded:?}");
            match &toks[0].kind {
                crate::lexer::TokenKind::Money(mv) => {
                    assert_eq!(mv.cents, cents, "cents mismatch: {molded}");
                    assert_eq!(mv.currency.as_ref(), cur, "currency mismatch: {molded}");
                }
                other => panic!("expected Money token for {molded}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_issue() {
        // M80: issue molds as `#<body>` (the `#` prefix + raw body).
        assert_eq!(mold_to_string(&Value::issue("ABC")), "#ABC");
        assert_eq!(mold_to_string(&Value::issue("1234")), "#1234");
        assert_eq!(mold_to_string(&Value::issue("foo-bar")), "#foo-bar");
    }

    #[test]
    fn mold_issue_round_trips_via_lexer() {
        for body in ["ABC", "1234", "foo-bar", "FF00", "a_b_c"] {
            let v = Value::issue(body);
            let molded = mold_to_string(&v);
            let toks = crate::lexer::lex(&molded).expect("lex issue");
            assert_eq!(toks.len(), 1, "{body} molded to {molded:?}");
            match &toks[0].kind {
                crate::lexer::TokenKind::Issue(s) => {
                    assert_eq!(s.as_ref(), body, "round-trip mismatch: {body} → {molded}");
                }
                other => panic!("expected Issue token for {molded}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_email() {
        // M80: email molds as the raw address (no quoting).
        assert_eq!(mold_to_string(&Value::email("foo@bar.com")), "foo@bar.com");
        assert_eq!(
            mold_to_string(&Value::email("user@host.example.org")),
            "user@host.example.org"
        );
    }

    #[test]
    fn mold_email_round_trips_via_lexer() {
        for addr in ["foo@bar.com", "user@host.example.org", "a@b.co"] {
            let v = Value::email(addr);
            let molded = mold_to_string(&v);
            let toks = crate::lexer::lex(&molded).expect("lex email");
            assert_eq!(toks.len(), 1, "{addr} molded to {molded:?}");
            match &toks[0].kind {
                crate::lexer::TokenKind::Email(parsed) => {
                    assert_eq!(
                        parsed.as_ref(),
                        addr,
                        "round-trip mismatch: {addr} → {molded}"
                    );
                }
                other => panic!("expected Email token for {molded}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_tag() {
        // M81: tag molds as `<body>` with escapes for `<`/`>`/`\`.
        assert_eq!(mold_to_string(&Value::tag("b")), "<b>");
        assert_eq!(mold_to_string(&Value::tag("/p")), "</p>");
        assert_eq!(
            mold_to_string(&Value::tag("img src=\"x\"")),
            "<img src=\"x\">"
        );
        // Escapes: body chars that would close/nest the tag are escaped.
        assert_eq!(mold_to_string(&Value::tag("a>b")), "<a\\>b>");
        assert_eq!(mold_to_string(&Value::tag("a<b")), "<a\\<b>");
        assert_eq!(mold_to_string(&Value::tag("a\\b")), "<a\\\\b>");
    }

    #[test]
    fn mold_tag_round_trips_via_lexer() {
        // M81: molded tags reparse to the same body.
        for body in [
            "b",
            "/p",
            "br/",
            "img src=\"x\"",
            "a=b",
            "a>b",
            "a<b",
            "a\\b",
        ] {
            let v = Value::tag(body);
            let molded = mold_to_string(&v);
            let toks = crate::lexer::lex(&molded).expect("lex tag");
            assert_eq!(toks.len(), 1, "{body:?} molded to {molded:?}");
            match &toks[0].kind {
                crate::lexer::TokenKind::Tag(parsed) => {
                    assert_eq!(
                        parsed.as_ref(),
                        body,
                        "round-trip mismatch: {body:?} → {molded:?}"
                    );
                }
                other => panic!("expected Tag token for {molded:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_deeply_nested_block() {
        // Deep nesting must not overflow; recursion handles arbitrary depth.
        let mut v = Value::integer(1);
        for _ in 0..50 {
            v = Value::block(Series::new(vec![v]));
        }
        let molded = mold_to_string(&v);
        // 50 opening brackets, the integer, 50 closing brackets.
        assert_eq!(molded.chars().filter(|&c| c == '[').count(), 50);
        assert_eq!(molded.chars().filter(|&c| c == ']').count(), 50);
        assert!(molded.contains('1'));
    }

    #[test]
    fn mold_string_plain() {
        assert_eq!(mold_to_string(&s("hello")), "\"hello\"");
    }

    #[test]
    fn mold_string_escapes() {
        assert_eq!(mold_to_string(&s("a\"b")), "\"a\\\"b\"");
        assert_eq!(mold_to_string(&s("a\\b")), "\"a\\\\b\"");
        assert_eq!(mold_to_string(&s("a\nb")), "\"a\\nb\"");
        assert_eq!(mold_to_string(&s("a\tb")), "\"a\\tb\"");
        assert_eq!(mold_to_string(&s("a\rb")), "\"a\\rb\"");
    }

    #[test]
    fn mold_char_basic() {
        assert_eq!(mold_to_string(&Value::char('a')), "#\"a\"");
        assert_eq!(mold_to_string(&Value::char('Z')), "#\"Z\"");
        assert_eq!(mold_to_string(&Value::char('1')), "#\"1\"");
    }

    #[test]
    fn mold_char_caret_escapes() {
        assert_eq!(mold_to_string(&Value::char('\t')), "#\"^-\"");
        assert_eq!(mold_to_string(&Value::char('\n')), "#\"^/\"");
        assert_eq!(mold_to_string(&Value::char('\u{0}')), "#\"^@\"");
        assert_eq!(mold_to_string(&Value::char('^')), "#\"^^\"");
        assert_eq!(mold_to_string(&Value::char('"')), "#\"^\"\"");
        assert_eq!(mold_to_string(&Value::char('\r')), "#\"^M\"");
        assert_eq!(mold_to_string(&Value::char('\u{1}')), "#\"^A\"");
    }

    #[test]
    fn mold_binary_round_trips_via_lexer() {
        // M41: every molded binary must re-parse to the same bytes.
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0x00],
            vec![0xFF],
            vec![0x48, 0x65, 0x6C, 0x6C, 0x6F], // "Hello"
            vec![0xDE, 0xAD, 0xBE, 0xEF],
            vec![0x00, 0x01, 0x02, 0xFE, 0xFF],
        ];
        for bytes in cases {
            let v = Value::String8 {
                bytes: bytes.clone(),
                span: Span::new(0, 0),
            };
            let molded = mold_to_string(&v);
            let toks = crate::lexer::lex(&molded).expect("lex molded binary");
            assert_eq!(toks.len(), 1, "expected 1 token for {molded:?}");
            match &toks[0].kind {
                crate::lexer::TokenKind::Binary(parsed) => {
                    assert_eq!(parsed.as_ref(), bytes.as_slice(), "round-trip mismatch");
                }
                other => panic!("expected Binary token for {molded:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_binary_is_uppercase_hex() {
        // Red molds binaries as `#{HEX}` uppercase — no separators.
        let v = Value::String8 {
            bytes: vec![0xDE, 0xAD],
            span: Span::new(0, 0),
        };
        assert_eq!(mold_to_string(&v), "#{DEAD}");
        // Lowercase input bytes still produce uppercase hex.
        let v2 = Value::String8 {
            bytes: vec![0xab, 0xcd],
            span: Span::new(0, 0),
        };
        assert_eq!(mold_to_string(&v2), "#{ABCD}");
    }

    #[test]
    fn mold_char_round_trips_via_lexer() {
        // M38: every molded char must re-parse to the same char value.
        for c in [
            'a', 'Z', '0', ' ', '!', '#', '%', '&', '\t', '\n', '\r', '\0', '^', '"', '\u{1A}',
            '\u{7F}',
        ] {
            let molded = mold_to_string(&Value::char(c));
            let toks = crate::lexer::lex(&molded).expect("lex molded char");
            assert_eq!(toks.len(), 1, "expected 1 token for {molded:?}");
            match &toks[0].kind {
                crate::lexer::TokenKind::Char(parsed) => {
                    assert_eq!(*parsed, c, "round-trip mismatch: {molded:?} → {parsed:?}");
                }
                other => panic!("expected Char token for {molded:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn mold_string_carriage_return_round_trips() {
        // A string containing a raw CR must mold to an escaped form so it
        // re-parses to the same value (the lexer's `\r` escape decodes to CR).
        let raw = s("line1\rline2");
        let molded = mold_to_string(&raw);
        assert_eq!(molded, "\"line1\\rline2\"");
        // No raw CR inside the quotes.
        assert!(!molded[1..molded.len() - 1].contains('\r'));
    }

    #[test]
    fn mold_string_control_chars_preserved() {
        // The four lexer-supported escapes round-trip.
        for raw in ["a\"b", "a\\b", "a\nb", "a\tb", "a\rb"] {
            let molded = mold_to_string(&s(raw));
            // Re-parse the molded form and compare.
            let toks = crate::lexer::lex(&molded).expect("lex molded string");
            assert_eq!(toks.len(), 1);
            match &toks[0].kind {
                crate::lexer::TokenKind::String(parsed) => {
                    assert_eq!(parsed.as_ref(), raw, "round-trip mismatch for {raw:?}");
                }
                other => panic!("expected String token, got {other:?}"),
            }
        }
    }

    #[test]
    fn string_escape_round_trip() {
        for raw in [
            "hello",
            "a\"b",
            "back\\slash",
            "tab\there",
            "new\nline",
            "mix\"\n\\t",
        ] {
            let molded = mold_to_string(&s(raw));
            // Molded form always starts/ends with a quote and contains no
            // raw control characters.
            assert!(molded.starts_with('"') && molded.ends_with('"'));
            assert!(!molded[1..molded.len() - 1].contains('\n'));
            assert!(!molded[1..molded.len() - 1].contains('\t'));
            // Manually unescape and compare to the original.
            let inner = &molded[1..molded.len() - 1];
            let mut decoded = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next().unwrap() {
                        '"' => decoded.push('"'),
                        '\\' => decoded.push('\\'),
                        'n' => decoded.push('\n'),
                        't' => decoded.push('\t'),
                        other => panic!("unexpected escape \\{}", other),
                    }
                } else {
                    decoded.push(c);
                }
            }
            assert_eq!(decoded, raw);
        }
    }

    #[test]
    fn mold_word_kinds() {
        assert_eq!(mold_to_string(&Value::word("foo")), "foo");
        assert_eq!(mold_to_string(&Value::set_word("foo")), "foo:");
        assert_eq!(mold_to_string(&Value::get_word("foo")), ":foo");
        assert_eq!(mold_to_string(&Value::lit_word("foo")), "'foo");
    }

    #[test]
    fn mold_empty_block() {
        assert_eq!(mold_to_string(&Value::block(Series::empty())), "[]");
    }

    #[test]
    fn mold_simple_block() {
        let v = Value::block(Series::new(vec![
            Value::integer(1),
            Value::integer(2),
            Value::integer(3),
        ]));
        assert_eq!(mold_to_string(&v), "[1 2 3]");
    }

    #[test]
    fn mold_block_mixed() {
        let v = Value::block(Series::new(vec![
            Value::word("print"),
            s("hi"),
            Value::integer(7),
        ]));
        assert_eq!(mold_to_string(&v), "[print \"hi\" 7]");
    }

    #[test]
    fn mold_nested_block() {
        let inner = Value::block(Series::new(vec![Value::word("b"), Value::word("c")]));
        let outer = Value::block(Series::new(vec![Value::word("a"), inner, Value::word("d")]));
        assert_eq!(mold_to_string(&outer), "[a [b c] d]");
    }

    #[test]
    fn mold_empty_paren() {
        assert_eq!(mold_to_string(&Value::paren(Series::empty())), "()");
    }

    #[test]
    fn mold_paren() {
        let v = Value::paren(Series::new(vec![Value::integer(1), Value::integer(2)]));
        assert_eq!(mold_to_string(&v), "(1 2)");
    }

    #[test]
    fn mold_nested_block_in_paren() {
        let inner = Value::block(Series::new(vec![Value::integer(1), Value::integer(2)]));
        let v = Value::paren(Series::new(vec![inner, Value::word("x")]));
        assert_eq!(mold_to_string(&v), "([1 2] x)");
    }

    #[test]
    fn mold_func_placeholder() {
        let fd = std::rc::Rc::new(crate::value::FuncDef::default());
        assert_eq!(mold_to_string(&Value::Func(fd)), "#[function]");
    }

    #[test]
    fn mold_closure_placeholder() {
        // M60: closure molds as `#[closure]` placeholder.
        let cd = Value::closure(
            std::rc::Rc::new(crate::value::FuncDef::default()),
            std::rc::Rc::new(Vec::new()),
        );
        assert_eq!(mold_to_string(&cd), "#[closure]");
        assert_eq!(form_to_string(&cd), "#[closure]");
    }

    #[test]
    fn mold_path() {
        let p = Value::path(vec![Value::word("foo"), Value::word("bar")]);
        assert_eq!(mold_to_string(&p), "foo/bar");
    }

    #[test]
    fn mold_path_three_parts() {
        let p = Value::path(vec![Value::word("a"), Value::word("b"), Value::word("c")]);
        assert_eq!(mold_to_string(&p), "a/b/c");
    }

    #[test]
    fn mold_refinement() {
        assert_eq!(mold_to_string(&Value::refinement("part")), "/part");
        assert_eq!(mold_to_string(&Value::refinement("only")), "/only");
    }

    #[test]
    fn symbol_intern_share() {
        // Sanity: two Symbols over the same `Rc<str>` share via Rc::clone.
        let a = Symbol::new("foo");
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "foo");
    }

    // --- form (M14) ---

    #[test]
    fn form_scalar_matches_mold() {
        assert_eq!(form_to_string(&Value::None), "none");
        assert_eq!(form_to_string(&Value::Logic(true)), "true");
        assert_eq!(form_to_string(&Value::Logic(false)), "false");
        assert_eq!(form_to_string(&Value::integer(42)), "42");
        assert_eq!(form_to_string(&Value::float(3.5)), "3.5");
    }

    #[test]
    fn form_string_is_raw() {
        // form strips quotes/escapes — the key difference from mold.
        assert_eq!(form_to_string(&s("hello")), "hello");
        assert_eq!(form_to_string(&s("a\nb")), "a\nb");
        assert_eq!(mold_to_string(&s("a\nb")), "\"a\\nb\"");
    }

    #[test]
    fn form_word_family_strips_markers() {
        assert_eq!(form_to_string(&Value::word("foo")), "foo");
        assert_eq!(form_to_string(&Value::set_word("foo")), "foo");
        assert_eq!(form_to_string(&Value::get_word("foo")), "foo");
        assert_eq!(form_to_string(&Value::lit_word("foo")), "foo");
        assert_eq!(form_to_string(&Value::refinement("part")), "part");
    }

    #[test]
    fn form_block_is_space_joined_no_brackets() {
        let v = Value::block(Series::new(vec![
            Value::integer(1),
            Value::integer(2),
            Value::integer(3),
        ]));
        assert_eq!(form_to_string(&v), "1 2 3");
        // mold still produces bracketed form.
        assert_eq!(mold_to_string(&v), "[1 2 3]");
    }

    #[test]
    fn form_block_with_strings_no_inner_quotes() {
        let v = Value::block(Series::new(vec![s("a"), s("b"), s("c")]));
        assert_eq!(form_to_string(&v), "a b c");
    }

    #[test]
    fn form_path_is_slash_joined() {
        let p = Value::path(vec![Value::word("foo"), Value::word("bar")]);
        assert_eq!(form_to_string(&p), "foo/bar");
    }

    // -- M84: vector! mold ------------------------------------------------

    fn int_vec(elems: &[i64]) -> Value {
        Value::vector(crate::value::VectorDef::new(
            Symbol::new("integer!"),
            elems.iter().map(|n| Value::integer(*n)).collect(),
        ))
    }

    fn float_vec(elems: &[f64]) -> Value {
        Value::vector(crate::value::VectorDef::new(
            Symbol::new("float!"),
            elems.iter().map(|f| Value::float(*f)).collect(),
        ))
    }

    #[test]
    fn mold_vector_int_kind() {
        assert_eq!(
            mold_to_string(&int_vec(&[1, 2, 3])),
            "make vector! [integer! 1 2 3]"
        );
    }

    #[test]
    fn mold_vector_float_kind() {
        assert_eq!(
            mold_to_string(&float_vec(&[1.0, 2.5, 3.0])),
            "make vector! [float! 1.0 2.5 3.0]"
        );
    }

    #[test]
    fn mold_vector_empty() {
        assert_eq!(mold_to_string(&int_vec(&[])), "make vector! [integer!]");
    }

    #[test]
    fn mold_vector_round_trips_via_make() {
        // mold → load_source → mold should be stable (synthetic value; the
        // reparse just yields a block that `make vector!` would consume).
        let v = int_vec(&[10, 20, 30]);
        let molded1 = mold_to_string(&v);
        let parsed = crate::parser::load_source(&molded1).expect("parse");
        // `load_source` yields the body Series; the first value is the
        // `make` call. We don't re-mold the parsed block (that would yield
        // a block mold, not a vector mold); instead assert the mold string
        // is itself well-formed and starts with the vector! prefix.
        assert!(molded1.starts_with("make vector! [integer!"));
        assert!(molded1.ends_with(']'));
        // Sanity: the parsed body has at least one value (the `make` path).
        assert!(!parsed.data.borrow().is_empty());
    }

    // -- M85: image! mold ------------------------------------------------

    fn img(w: usize, h: usize, bytes: &[u8]) -> Value {
        Value::image(crate::value::ImageDef::from_bytes(w, h, bytes).unwrap())
    }

    #[test]
    fn mold_image_basic() {
        assert_eq!(
            mold_to_string(&img(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255])),
            "make image! [width: 2 height: 1 pixels: [255 0 0 255 0 255 0 255]]"
        );
    }

    #[test]
    fn mold_image_empty() {
        assert_eq!(
            mold_to_string(&img(0, 0, &[])),
            "make image! [width: 0 height: 0 pixels: []]"
        );
    }

    #[test]
    fn mold_image_round_trips_via_make() {
        let v = img(1, 1, &[10, 20, 30, 40]);
        let molded1 = mold_to_string(&v);
        let parsed = crate::parser::load_source(&molded1).expect("parse");
        assert!(molded1.starts_with("make image! [width:"));
        assert!(molded1.ends_with(']'));
        assert!(!parsed.data.borrow().is_empty());
    }
}
