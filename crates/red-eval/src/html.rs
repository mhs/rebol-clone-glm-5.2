//! M158–M159: HTML builder dialect — `html [...]`.
//!
//! A flat-block dialect that parses HTML from `tag!` literals, `string!`
//! content, and `paren!` expressions. The tag open/close structure defines
//! nesting — no nested blocks required.
//!
//! Grammar:
//!   html [
//!       <div class="main">        ; opening tag with attributes
//!           <h1> "Welcome" </h1>  ; text content + closing tag
//!           <p> "Hello, " <b> "World" </b> "!" </p>
//!           <img src="x.png">      ; void element (self-closing)
//!           <br>                  ; void element
//!       </div>                    ; closing tag
//!   ]
//!
//! Attributes are parsed from the tag body string. Paren interpolation is
//! supported: `<a href=(url)>` evaluates `(url)` as Red code and uses the
//! result as the attribute value.
//!
//! Refinements: `/xml` (no void elements — all tags need closing),
//! `/raw` (no HTML-escaping of text content), `/indent` (pretty-print).

use std::rc::Rc;

use red_core::parser::load_source;
use red_core::printer::form_to_string;
use red_core::value::{Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp::dispatch_block;
use crate::natives::{reg_refined, type_name};
use crate::NativeFn;

/// HTML5 void elements that self-close (no closing tag needed).
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
    "param", "source", "track", "wbr",
];

/// Rendering options derived from the refinements.
struct HtmlOpts {
    xml_mode: bool,
    raw: bool,
    pretty: bool,
    indent_width: usize,
}

/// `html block` / `html/xml block` / `html/raw block` / `html/indent block`.
pub fn build_html_native(args: &[Value], refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("html"),
            expected: 1,
            got: 0,
            span: Span::default(),
        });
    }
    let block = &args[0];
    let span = block.span_or_default();
    let series = match block {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: type_name(other),
                span,
            });
        }
    };

    let opts = HtmlOpts {
        xml_mode: refs.has(&Symbol::new("xml")),
        raw: refs.has(&Symbol::new("raw")),
        pretty: refs.has(&Symbol::new("indent")),
        indent_width: 2,
    };

    let data = series.data.borrow();
    let elems: Vec<Value> = data.iter().skip(series.index).cloned().collect();
    drop(data);

    let mut out = String::new();
    render_tokens(&elems, &mut out, 0, env, &opts, span)?;
    Ok(Value::string(Rc::from(out.as_str())))
}

// ===========================================================================
// Flat token-stream renderer — builds HTML from the open/close tag structure
// ===========================================================================

/// Render a flat sequence of tokens (tags, strings, parens, blocks). The tag
/// open/close structure defines nesting via a stack.
fn render_tokens(
    elems: &[Value],
    out: &mut String,
    indent: usize,
    env: &mut Env,
    opts: &HtmlOpts,
    span: Span,
) -> Result<(), EvalError> {
    // Stack of open tag names (for matching closing tags).
    let mut stack: Vec<String> = Vec::new();

    for elem in elems {
        match elem {
            // Tag literal: opening, closing, or self-closing.
            Value::Tag { text, .. } => {
                let body = text.as_ref();
                if body.starts_with('/') {
                    // Closing tag: </div>
                    let close_name = body[1..].trim();
                    if let Some(open_name) = stack.pop() {
                        let cur_indent = indent + stack.len();
                        if opts.pretty {
                            push_indent(out, cur_indent, opts.indent_width);
                        }
                        out.push_str("</");
                        out.push_str(close_name);
                        out.push('>');
                        if opts.pretty {
                            out.push('\n');
                        }
                        if open_name != close_name {
                            // Mismatch — auto-correct by closing all open tags
                            // until we find a match (browser-like leniency).
                            // For now, just warn silently.
                        }
                    } else {
                        // Stray closing tag — ignore (browser-like leniency).
                    }
                } else {
                    // Opening or self-closing tag.
                    let parsed = parse_tag_body(body, env, span)?;
                    let is_void =
                        !opts.xml_mode && (VOID_ELEMENTS.contains(&parsed.name.as_str())
                            || parsed.self_closing);

                    let cur_indent = indent + stack.len();
                    if opts.pretty {
                        push_indent(out, cur_indent, opts.indent_width);
                    }
                    out.push('<');
                    out.push_str(&parsed.name);
                    for (key, val) in &parsed.attrs {
                        out.push(' ');
                        out.push_str(key);
                        if !val.is_empty() {
                            out.push_str("=\"");
                            escape_attr(val, out);
                            out.push('"');
                        }
                    }
                    if is_void {
                        out.push_str(" />");
                        if opts.pretty {
                            out.push('\n');
                        }
                    } else {
                        out.push('>');
                        if opts.pretty {
                            out.push('\n');
                        }
                        stack.push(parsed.name.clone());
                    }
                }
            }
            // String: text content (HTML-escaped unless /raw).
            Value::String { s, .. } => {
                let cur_indent = indent + stack.len();
                if opts.pretty {
                    push_indent(out, cur_indent, opts.indent_width);
                }
                if opts.raw {
                    out.push_str(s);
                } else {
                    html_escape(s, out);
                }
                if opts.pretty {
                    out.push('\n');
                }
            }
            // Paren: evaluate Red code, form result, append (escaped unless /raw).
            Value::Paren { series, .. } => {
                let result = dispatch_block(&Value::paren(series.clone()), env)?;
                let text = form_to_string(&result);
                let cur_indent = indent + stack.len();
                if opts.pretty {
                    push_indent(out, cur_indent, opts.indent_width);
                }
                if opts.raw {
                    out.push_str(&text);
                } else {
                    html_escape(&text, out);
                }
                if opts.pretty {
                    out.push('\n');
                }
            }
            // Nested block: recurse as a transparent token group.
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let nested: Vec<Value> = data.iter().skip(series.index).cloned().collect();
                drop(data);
                // Recurse at the current stack depth — the nested block's
                // tags continue the flat token stream.
                render_tokens(&nested, out, indent, env, opts, span)?;
            }
            // Anything else: form it and treat as text content.
            other => {
                let text = form_to_string(other);
                let cur_indent = indent + stack.len();
                if opts.pretty {
                    push_indent(out, cur_indent, opts.indent_width);
                }
                if opts.raw {
                    out.push_str(&text);
                } else {
                    html_escape(&text, out);
                }
                if opts.pretty {
                    out.push('\n');
                }
            }
        }
    }

    // Auto-close any unclosed tags (browser-like leniency).
    while let Some(name) = stack.pop() {
        let cur_indent = indent + stack.len();
        if opts.pretty {
            push_indent(out, cur_indent, opts.indent_width);
        }
        out.push_str("</");
        out.push_str(&name);
        out.push('>');
        if opts.pretty {
            out.push('\n');
        }
    }

    Ok(())
}

// ===========================================================================
// Tag body parser — extracts name + attributes from the tag body string
// ===========================================================================

struct ParsedTag {
    name: String,
    attrs: Vec<(String, String)>,
    self_closing: bool,
}

/// Parse the tag body string (everything between `<` and `>`) into a tag name
/// and a list of attributes. Supports:
///   - Quoted values: `class="main"`
///   - Paren interpolation: `href=(url)` — evaluates `url` as Red code
///   - Boolean attributes: `defer` (no `=` or value)
///   - Self-closing: `br/` (trailing `/`)
fn parse_tag_body(body: &str, env: &mut Env, span: Span) -> Result<ParsedTag, EvalError> {
    let trimmed = body.trim();

    // Self-closing: trailing `/` (but not leading `/` which is a closing tag).
    let (inner, self_closing) = if trimmed.ends_with('/')
        && !trimmed.starts_with('/')
    {
        (trimmed[..trimmed.len() - 1].trim(), true)
    } else {
        (trimmed, false)
    };

    // Tag name: first token up to whitespace.
    let name_end = inner
        .find(|c: char| c.is_whitespace())
        .unwrap_or(inner.len());
    let name = inner[..name_end].to_string();
    if name.is_empty() {
        return Err(EvalError::Native {
            message: "html: empty tag name".into(),
            span,
        });
    }
    let rest = inner[name_end..].trim();

    // Parse attributes.
    let attrs = parse_attrs(rest, env, span)?;

    Ok(ParsedTag {
        name,
        attrs,
        self_closing,
    })
}

/// Parse attribute key=value pairs from the remainder of the tag body.
fn parse_attrs(
    rest: &str,
    env: &mut Env,
    span: Span,
) -> Result<Vec<(String, String)>, EvalError> {
    let mut attrs = Vec::new();
    let bytes = rest.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Skip whitespace.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        // Read attribute key (up to `=`, whitespace, or end).
        let key_start = i;
        while i < bytes.len()
            && bytes[i] != b'='
            && !bytes[i].is_ascii_whitespace()
        {
            i += 1;
        }
        let key = rest[key_start..i].to_string();

        // Check for `=value`.
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1; // consume `=`

            if i >= bytes.len() {
                // `=` at end — treat as boolean with empty value.
                attrs.push((key, String::new()));
                break;
            }

            let val = if bytes[i] == b'"' {
                // Quoted string value: read until closing `"`.
                i += 1; // consume opening quote
                let val_start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                let v = rest[val_start..i].to_string();
                if i < bytes.len() {
                    i += 1; // consume closing quote
                }
                v
            } else if bytes[i] == b'\'' {
                // Single-quoted value: read until closing `'`.
                i += 1;
                let val_start = i;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                let v = rest[val_start..i].to_string();
                if i < bytes.len() {
                    i += 1;
                }
                v
            } else if bytes[i] == b'(' {
                // Paren interpolation: find matching `)`, evaluate as Red.
                i += 1; // consume opening `(`
                let expr_start = i;
                let mut depth = 1;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    if depth > 0 {
                        i += 1;
                    }
                }
                let expr_text = &rest[expr_start..i];
                if i < bytes.len() {
                    i += 1; // consume closing `)`
                }

                // Evaluate the expression as Red code.
                eval_attr_expr(expr_text, env, span)?
            } else {
                // Unquoted bare value: read until whitespace.
                let val_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                rest[val_start..i].to_string()
            };

            attrs.push((key, val));
        } else {
            // Boolean attribute (no `=`).
            attrs.push((key, String::new()));
        }
    }

    Ok(attrs)
}

/// Evaluate a Red expression from a string (used for `=(expr)` attribute
/// interpolation). Parses via `load_source`, binds against `user_ctx`, and
/// evaluates. Returns the `form`'d result as a string.
fn eval_attr_expr(src: &str, env: &mut Env, span: Span) -> Result<String, EvalError> {
    let body = load_source(src).map_err(|e| EvalError::Native {
        message: format!("html: failed to parse attribute expression '{src}': {e}"),
        span,
    })?;
    crate::binding::bind_pass_into(&body, &env.user_ctx);
    let block = Value::block(body);
    let result = dispatch_block(&block, env)?;
    Ok(form_to_string(&result))
}

// ===========================================================================
// Escaping helpers
// ===========================================================================

/// HTML-escape text content: `&` → `&amp;`, `<` → `&lt;`, `>` → `&gt;`,
/// `"` → `&quot;`.
fn html_escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// Escape an attribute value: `"` → `&quot;`, `&` → `&amp;`. Does NOT escape
/// `<`/`>` (they're safe inside quoted attributes).
fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("&quot;"),
            '&' => out.push_str("&amp;"),
            c => out.push(c),
        }
    }
}

/// Write `n` levels of indentation, each `indent_width` spaces.
fn push_indent(out: &mut String, n: usize, indent_width: usize) {
    for _ in 0..(n * indent_width) {
        out.push(' ');
    }
}

// ===========================================================================
// Registration
// ===========================================================================

pub fn register_html_natives(env: &mut Env) {
    reg_refined(
        env,
        "html",
        build_html_native as NativeFn,
        1,
        &[("xml", 0), ("raw", 0), ("indent", 0)],
    );
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::register_html_natives;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives, type_name};
    use crate::eval;
    use crate::json::register_json_natives;
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::value::Value;
    use red_core::{Env, EvalError};
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    struct BufferWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn run_capture_val(src: &str) -> Result<(Value, Vec<u8>), String> {
        let body = load_source(src).map_err(|e| e.to_string())?;
        let ctx = Context::new();
        install_constants(&ctx);
        let ctx_rc = bind_pass(&body, ctx);
        let buf = Rc::new(RefCell::new(Vec::<u8>::new()));
        let mut env = Env::new_with_output(ctx_rc, Box::new(BufferWriter(Rc::clone(&buf))));
        register_natives(&mut env);
        register_html_natives(&mut env);
        register_json_natives(&mut env);
        let block = Value::block(body);
        let val = match eval(&block, &mut env) {
            Ok(v) => v,
            Err(EvalError::Quit(_)) => Value::None,
            Err(e) => return Err(e.to_string()),
        };
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    #[allow(dead_code)]
    fn out(src: &str) -> String {
        s(&run_capture_val(src).unwrap().1)
    }

    fn str_val(v: &Value) -> String {
        match v {
            Value::String { s, .. } => (**s).to_string(),
            other => panic!("expected string!, got {}", type_name(other)),
        }
    }

    // --- Basic tag-based tests ---

    #[test]
    fn html_simple_tag() {
        let v = val("html [<p> \"Hello\" </p>]");
        assert_eq!(str_val(&v), "<p>Hello</p>");
    }

    #[test]
    fn html_attrs_in_tag() {
        let v = val("html [<div class=\"main\"> \"text\" </div>]");
        assert_eq!(str_val(&v), "<div class=\"main\">text</div>");
    }

    #[test]
    fn html_nested_tags() {
        let v = val("html [<ul> <li> \"a\" </li> <li> \"b\" </li> </ul>]");
        assert_eq!(str_val(&v), "<ul><li>a</li><li>b</li></ul>");
    }

    #[test]
    fn html_void_element() {
        let v = val("html [<br>]");
        assert_eq!(str_val(&v), "<br />");
    }

    #[test]
    fn html_void_with_attr() {
        let v = val("html [<img src=\"x.png\">]");
        assert_eq!(str_val(&v), "<img src=\"x.png\" />");
    }

    #[test]
    fn html_self_closing_tag() {
        let v = val("html [<br/>]");
        assert_eq!(str_val(&v), "<br />");
    }

    #[test]
    fn html_text_escape() {
        let v = val("html [<p> {<script>alert(1)</script>} </p>]");
        assert_eq!(
            str_val(&v),
            "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>"
        );
    }

    #[test]
    fn html_paren_eval() {
        let v = val("name: \"World\" html [<p> (\"Hello \" + name) </p>]");
        assert_eq!(str_val(&v), "<p>Hello World</p>");
    }

    #[test]
    fn html_boolean_attr() {
        let v = val("html [<script defer src=\"app.js\"> </script>]");
        assert_eq!(str_val(&v), "<script defer src=\"app.js\"></script>");
    }

    #[test]
    fn html_auto_close() {
        // Unclosed tags are auto-closed (browser-like leniency).
        let v = val("html [<div> <p> \"text\"]");
        assert_eq!(str_val(&v), "<div><p>text</p></div>");
    }

    #[test]
    fn html_xml_mode_void() {
        let v = val("html/xml [<br> </br>]");
        assert_eq!(str_val(&v), "<br></br>");
    }

    #[test]
    fn html_raw_mode() {
        let v = val("html/raw [<p> {<b>bold</b>} </p>]");
        assert_eq!(str_val(&v), "<p><b>bold</b></p>");
    }

    #[test]
    fn html_multiple_top_level() {
        let v = val("html [<h1> \"Title\" </h1> <p> \"Para\" </p>]");
        assert_eq!(str_val(&v), "<h1>Title</h1><p>Para</p>");
    }

    #[test]
    fn html_deeply_nested() {
        let v = val("html [<div> <div> <div> \"deep\" </div> </div> </div>]");
        assert_eq!(str_val(&v), "<div><div><div>deep</div></div></div>");
    }

    #[test]
    fn html_attr_paren_interpolation() {
        let v = val("url: \"http://example.com\" html [<a href=(url)> \"link\" </a>]");
        assert_eq!(str_val(&v), "<a href=\"http://example.com\">link</a>");
    }

    #[test]
    fn html_attr_paren_expr() {
        let v = val("html [<div id=(rejoin [\"user-\" 42])> \"content\" </div>]");
        assert_eq!(str_val(&v), "<div id=\"user-42\">content</div>");
    }

    #[test]
    fn html_print_output() {
        let result = out("print html [<p> \"Hello\" </p>]");
        assert_eq!(result, "<p>Hello</p>\n");
    }

    #[test]
    fn html_mixed_content() {
        let v = val("html [<p> \"Hello, \" <b> \"World\" </b> \"!\" </p>]");
        assert_eq!(str_val(&v), "<p>Hello, <b>World</b>!</p>");
    }

    #[test]
    fn html_full_page() {
        let v = val("html [<html> <head> <title> \"My Page\" </title> </head> <body> <h1> \"Welcome\" </h1> </body> </html>]");
        assert_eq!(
            str_val(&v),
            "<html><head><title>My Page</title></head><body><h1>Welcome</h1></body></html>"
        );
    }
}
