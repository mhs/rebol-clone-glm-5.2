//! M158–M159: HTML builder dialect — `html [...]`.
//!
//! A block-walking dialect that assembles HTML/XML from a nested block of
//! tag words, attribute pairs, string content, and paren-embedded Red code.
//! Returns a `string!` with the rendered markup.
//!
//! Grammar:
//!   html [
//!       tag-name [attr-val attr-val ... children ...]
//!       "text content"          ; HTML-escaped
//!       (red-expression)        ; evaluated, form'd, HTML-escaped
//!       nested-block            ; recursed
//!   ]
//!
//! Void elements (`br`, `img`, `hr`, …) self-close.
//! Refinements: `/xml` (XML mode — no void elements), `/raw` (no escaping),
//! `/indent N` (pretty-print with N-space indent).

use std::rc::Rc;

use red_core::printer::form_to_string;
use red_core::value::{Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp::dispatch_block;
use crate::natives::{reg_refined, type_name};
use crate::series::word_sym;
use crate::NativeFn;

/// HTML5 void elements that self-close (no closing tag, no children).
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
    "param", "source", "track", "wbr",
];

/// Common HTML tag names — used to distinguish tags from attribute names when
/// a word follows an attribute key without a value. If the word is a known
/// tag, it starts a new element (the current attr is boolean); otherwise it's
/// treated as a boolean attribute.
const KNOWN_TAGS: &[&str] = &[
    "html", "head", "body", "div", "span", "p", "a", "img", "ul", "ol", "li",
    "table", "tr", "td", "th", "thead", "tbody", "tfoot", "br", "hr", "h1",
    "h2", "h3", "h4", "h5", "h6", "title", "meta", "link", "script", "style",
    "form", "input", "button", "label", "select", "option", "textarea",
    "nav", "header", "footer", "main", "section", "article", "aside", "figure",
    "figcaption", "blockquote", "pre", "code", "em", "strong", "b", "i", "u",
    "small", "sub", "sup", "dl", "dt", "dd", "caption", "col", "colgroup",
    "fieldset", "legend", "noscript", "iframe", "canvas", "svg", "video",
    "audio", "source", "track", "embed", "object", "param", "wbr", "base",
    "area", "map",
];

fn is_known_tag(name: &str) -> bool {
    KNOWN_TAGS.contains(&name)
}

/// Rendering options derived from the refinements.
struct HtmlOpts {
    xml_mode: bool,
    raw: bool,
    pretty: bool,
    indent_width: usize,
}

/// `html block` / `html/xml block` / `html/raw block` / `html/indent block N`.
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
        indent_width: {
            if let Some(vals) = refs.get(&Symbol::new("indent")) {
                match vals.first() {
                    Some(Value::Integer { n, .. }) => *n.max(&0) as usize,
                    _ => 2,
                }
            } else {
                2
            }
        },
    };

    let data = series.data.borrow();
    let elems: Vec<Value> = data.iter().skip(series.index).cloned().collect();
    drop(data);

    let mut out = String::new();
    render_block(&elems, &mut out, 0, env, &opts, span)?;
    Ok(Value::string(Rc::from(out.as_str())))
}

/// Render a flat sequence of block elements (the top-level block or a child
/// block). Each element is either a tag word, a string, a paren, a nested
/// block, or an attribute value belonging to the preceding tag word.
fn render_block(
    elems: &[Value],
    out: &mut String,
    indent: usize,
    env: &mut Env,
    opts: &HtmlOpts,
    span: Span,
) -> Result<(), EvalError> {
    let mut i = 0;
    while i < elems.len() {
        let v = &elems[i];
        match v {
            // Tag word: `div`, `h1`, `br` — start of an HTML element.
            Value::Word { sym, .. }
            | Value::SetWord { sym, .. }
            | Value::GetWord { sym, .. }
            | Value::LitWord { sym, .. } => {
                let tag_name = sym.as_str().to_string();
                i += 1;
                render_tag(&tag_name, elems, &mut i, out, indent, env, opts, span)?;
            }
            // String: text content (HTML-escaped unless /raw).
            Value::String { s, .. } => {
                if opts.pretty {
                    push_indent(out, indent, opts.indent_width);
                }
                if opts.raw {
                    out.push_str(s);
                } else {
                    html_escape(s, out);
                }
                i += 1;
            }
            // Paren: evaluate Red code, form result, append (escaped unless /raw).
            Value::Paren { series, .. } => {
                let result = dispatch_block(&Value::paren(series.clone()), env)?;
                let text = form_to_string(&result);
                if opts.pretty {
                    push_indent(out, indent, opts.indent_width);
                }
                if opts.raw {
                    out.push_str(&text);
                } else {
                    html_escape(&text, out);
                }
                i += 1;
            }
            // Nested block: recurse.
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let nested: Vec<Value> = data.iter().skip(series.index).cloned().collect();
                drop(data);
                render_block(&nested, out, indent, env, opts, span)?;
                i += 1;
            }
            // Anything else: form it and treat as text content.
            other => {
                let text = form_to_string(other);
                if opts.raw {
                    out.push_str(&text);
                } else {
                    html_escape(&text, out);
                }
                i += 1;
            }
        }
    }
    Ok(())
}

/// Render a single tag, collecting attributes from the following elements
/// (word + optional value pairs), then collecting inline children (all
/// non-word elements until the next word or end of block).
fn render_tag(
    tag_name: &str,
    elems: &[Value],
    i: &mut usize,
    out: &mut String,
    indent: usize,
    env: &mut Env,
    opts: &HtmlOpts,
    span: Span,
) -> Result<(), EvalError> {
    let is_void = !opts.xml_mode && VOID_ELEMENTS.contains(&tag_name);

    if opts.pretty {
        push_indent(out, indent, opts.indent_width);
    }
    out.push('<');
    out.push_str(tag_name);

    // Collect attributes: word + optional value. A word followed by another
    // word or a block is a boolean attribute. A word that is itself a known
    // tag name starts a new element (don't treat it as an attribute).
    while *i < elems.len() {
        let attr_key = match word_sym(&elems[*i]) {
            Some(s) => s.as_str().to_string(),
            None => break, // Not a word — exit to children mode.
        };
        // If this word is a known tag name, it's a new tag — don't consume
        // it as an attribute; exit the attr loop.
        if VOID_ELEMENTS.contains(&attr_key.as_str()) || is_known_tag(&attr_key) {
            break;
        }
        *i += 1;

        // Peek: is the next element a value (attribute value) or a new tag/block?
        if *i < elems.len() {
            let next = &elems[*i];
            match next {
                // Next is a block → boolean attr, and the block is children.
                Value::Block { .. } => {
                    push_attr(out, &attr_key, None, opts);
                    break;
                }
                // Next is a word that's a known tag name → the current attr
                // is boolean (if not already set), and the word starts a new
                // tag. Don't push the boolean attr — just break so the word
                // is processed as a new tag.
                Value::Word { sym, .. }
                | Value::SetWord { sym, .. }
                | Value::GetWord { sym, .. }
                | Value::LitWord { sym, .. } => {
                    if VOID_ELEMENTS.contains(&sym.as_str())
                        || is_known_tag(sym.as_str())
                    {
                        // This word starts a new tag — current attr is boolean.
                        push_attr(out, &attr_key, None, opts);
                        break;
                    }
                    // Unknown word — treat as boolean attribute.
                    push_attr(out, &attr_key, None, opts);
                    // Don't consume; loop re-processes the word as next attr key.
                }
                // Next is a value (string, integer, paren, etc.) — attr value.
                _ => {
                    let value_str = if let Value::Paren { series, .. } = next {
                        let result = dispatch_block(&Value::paren(series.clone()), env)?;
                        form_to_string(&result)
                    } else {
                        form_to_string(next)
                    };
                    push_attr(out, &attr_key, Some(&value_str), opts);
                    *i += 1;
                }
            }
        } else {
            // End of block — boolean attribute.
            push_attr(out, &attr_key, None, opts);
            break;
        }
    }

    if is_void {
        out.push_str(" />");
        if opts.pretty {
            out.push('\n');
        }
        return Ok(());
    }

    // Collect inline children: all non-word elements from the current position
    // until the next word (which starts a new tag) or end of block.
    let children_start = *i;
    while *i < elems.len() {
        if word_sym(&elems[*i]).is_some() {
            break; // next tag starts
        }
        *i += 1;
    }

    let children = &elems[children_start..*i];
    if children.is_empty() {
        out.push_str("></");
        out.push_str(tag_name);
        out.push('>');
        if opts.pretty {
            out.push('\n');
        }
        return Ok(());
    }

    out.push('>');
    if opts.pretty {
        out.push('\n');
    }
    render_block(children, out, indent + 1, env, opts, span)?;

    if opts.pretty {
        push_indent(out, indent, opts.indent_width);
    }
    out.push_str("</");
    out.push_str(tag_name);
    out.push('>');
    if opts.pretty {
        out.push('\n');
    }
    Ok(())
}

/// Write an attribute `key="value"` (or just `key` for boolean). Escapes the
/// value's `"` → `&quot;`.
fn push_attr(out: &mut String, key: &str, value: Option<&str>, opts: &HtmlOpts) {
    if opts.pretty {
        out.push(' ');
    } else {
        out.push(' ');
    }
    out.push_str(key);
    if let Some(v) = value {
        out.push('=');
        out.push('"');
        // Escape quotes in attribute values; don't escape `<`/`>` (inside quotes).
        for c in v.chars() {
            match c {
                '"' => out.push_str("&quot;"),
                '&' => out.push_str("&amp;"),
                c => out.push(c),
            }
        }
        out.push('"');
    }
}

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

    fn out(src: &str) -> String {
        s(&run_capture_val(src).unwrap().1)
    }

    fn str_val(v: &Value) -> String {
        match v {
            Value::String { s, .. } => (**s).to_string(),
            other => panic!("expected string!, got {}", type_name(other)),
        }
    }

    #[test]
    fn html_simple_tag() {
        let v = val("html [p \"Hello\"]");
        assert_eq!(str_val(&v), "<p>Hello</p>");
    }

    #[test]
    fn html_attributes() {
        let v = val("html [div class \"main\" \"text\"]");
        assert_eq!(str_val(&v), "<div class=\"main\">text</div>");
    }

    #[test]
    fn html_nested() {
        let v = val("html [ul [li \"a\" li \"b\"]]");
        assert_eq!(str_val(&v), "<ul><li>a</li><li>b</li></ul>");
    }

    #[test]
    fn html_void_element() {
        let v = val("html [br]");
        assert_eq!(str_val(&v), "<br />");
    }

    #[test]
    fn html_void_with_attr() {
        let v = val("html [img src \"x.png\"]");
        assert_eq!(str_val(&v), "<img src=\"x.png\" />");
    }

    #[test]
    fn html_text_escape() {
        let v = val("html [p {<script>alert(1)</script>}]");
        assert_eq!(
            str_val(&v),
            "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>"
        );
    }

    #[test]
    fn html_paren_eval() {
        let v = val("name: \"World\" html [p (\"Hello \" + name)]");
        assert_eq!(str_val(&v), "<p>Hello World</p>");
    }

    #[test]
    fn html_boolean_attr() {
        let v = val("html [script defer src \"app.js\"]");
        // `defer` is boolean, `src` has a value.
        assert_eq!(str_val(&v), "<script defer src=\"app.js\"></script>");
    }

    #[test]
    fn html_empty_children() {
        let v = val("html [div class \"box\" []]");
        assert_eq!(str_val(&v), "<div class=\"box\"></div>");
    }

    #[test]
    fn html_xml_mode_void() {
        let v = val("html/xml [br]");
        // XML mode: no void elements — `<br></br>`.
        assert_eq!(str_val(&v), "<br></br>");
    }

    #[test]
    fn html_raw_mode() {
        let v = val("html/raw [p {<b>bold</b>}]");
        assert_eq!(str_val(&v), "<p><b>bold</b></p>");
    }

    #[test]
    fn html_multiple_top_level() {
        let v = val("html [h1 \"Title\" p \"Para\"]");
        assert_eq!(str_val(&v), "<h1>Title</h1><p>Para</p>");
    }

    #[test]
    fn html_deeply_nested() {
        let v = val("html [div [div [div \"deep\"]]]");
        assert_eq!(str_val(&v), "<div><div><div>deep</div></div></div>");
    }

    #[test]
    fn html_integer_attr() {
        let v = val("html [td colspan 2 \"cell\"]");
        assert_eq!(str_val(&v), "<td colspan=\"2\">cell</td>");
    }

    #[test]
    fn html_pretty_indent() {
        let v = val("html/indent [div [p \"hi\"]]");
        let s = str_val(&v);
        assert!(s.contains("\n"), "pretty output should have newlines: {s}");
        assert!(s.contains("  <p>"), "pretty output should indent child tag: {s}");
        assert!(s.contains("    hi"), "pretty output should indent text: {s}");
    }

    #[test]
    fn html_print_output() {
        let result = out("print html [p \"Hello\"]");
        assert_eq!(result, "<p>Hello</p>\n");
    }
}
