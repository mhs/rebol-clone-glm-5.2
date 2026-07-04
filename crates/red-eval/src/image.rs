//! Images (M85): a fixed-size 2D RGBA8 pixel buffer. Pure data — no
//! GUI/draw surface. The second "container with a typed payload" type
//! (after `vector!`), but unlike `vector!` the size is fixed (no
//! `append`/`insert`/`remove`).
//!
//! Images are created via `make image! <spec>` (or `to-image`). The spec
//! may be:
//! - a `block!` keyword form: `[width: 100 height: 100 pixels: [...bytes...]]`.
//!   The `pixels:` value is a flat block of 0-255 integers (`width * height * 4`
//!   of them) or a `binary!` of the same length.
//! - a `block!` positional form: `[100 100 [...pixels...]]` — width, height,
//!   then the pixel block/binary.
//! - an `image!` → identity (clone the contents into a fresh `ImageDef`).
//!
//! Path resolution (`image/width`, `image/height`, `image/size` → `pair!`,
//! `image/<n>` → 1-based flat pixel pick as 4-byte `tuple!`, `image/<x>x<y>`
//! → 1-based coordinate pick) is in `interp_walker.rs`; the limited series
//! natives (`length?` → pixel count, `pick`, `poke`) are wired in `series.rs`;
//! equality is in `natives/compare.rs`; `same?`/`not-same?` are in `object.rs`.
//! `image!` is NOT a `series!` — `append`/`insert`/`remove`/etc. error since
//! the size is fixed.

use std::rc::Rc;

use red_core::value::{ImageDef, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp_walker::eval_expression;
use crate::natives::{arity_err, type_name};

// ---------------------------------------------------------------------------
// make image! / to-image
// ---------------------------------------------------------------------------

/// `make image! <spec>` — build a new `image!`. See module docs for spec
/// forms.
pub fn make_image(spec: &Value, env: &mut Env) -> Result<Value, EvalError> {
    match spec {
        Value::Block { series, span } => {
            let data = series.data.borrow();
            build_from_block(&data, series.index, *span, env)
        }
        Value::Image(im) => {
            // Shallow copy: new ImageDef with cloned dimensions + pixels.
            let b = im.borrow();
            let pixels = b.pixels.borrow().clone();
            Ok(Value::image(ImageDef::new(b.width, b.height, pixels)))
        }
        other => Err(EvalError::TypeError {
            expected: "block! or image!",
            found: type_name(other),
            span: other.span_or_default(),
        }),
    }
}

fn build_from_block(
    data: &std::cell::Ref<Vec<Value>>,
    start: usize,
    span: Span,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if data.len() == start {
        // Empty block — degenerate 0×0 image.
        return Ok(Value::image(ImageDef::empty(0, 0)));
    }
    // Try the keyword form first: scan for `width:` / `height:` / `pixels:`
    // set-words at the top level of the block. If we find any, dispatch to
    // the keyword parser; otherwise fall through to the positional form.
    let has_keyword = (start..data.len()).any(|i| {
        matches!(
            &data[i],
            Value::SetWord { sym, .. }
                if sym.as_str() == "width" || sym.as_str() == "height" || sym.as_str() == "pixels"
        )
    });
    if has_keyword {
        return build_keyword_form(data, start, span, env);
    }
    // Positional: `[width height pixels]`.
    build_positional_form(data, start, span, env)
}

fn build_keyword_form(
    data: &std::cell::Ref<Vec<Value>>,
    start: usize,
    span: Span,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let mut width: Option<i64> = None;
    let mut height: Option<i64> = None;
    let mut pixels: Option<Value> = None;
    let mut i = start;
    while i < data.len() {
        let head = &data[i];
        if let Value::SetWord { sym, .. } = head {
            i += 1;
            if i >= data.len() {
                return Err(EvalError::Native {
                    message: format!("make image!: {} is missing its value", sym.as_str()),
                    span,
                });
            }
            let v = eval_expression(data, &mut i, env)?;
            match sym.as_str() {
                "width" => {
                    width = Some(expect_int(&v, "width", span)?);
                }
                "height" => {
                    height = Some(expect_int(&v, "height", span)?);
                }
                "pixels" => {
                    pixels = Some(v);
                }
                _ => {
                    return Err(EvalError::Native {
                        message: format!(
                            "make image!: unknown keyword {} (expected width: height: pixels:)",
                            sym.as_str()
                        ),
                        span,
                    });
                }
            }
        } else {
            // Skip non-setword elements (allow trailing comments / none).
            i += 1;
        }
    }
    let w = width.unwrap_or(0).max(0) as usize;
    let h = height.unwrap_or(0).max(0) as usize;
    let px = pixels.unwrap_or(Value::Block {
        series: red_core::value::Series::new(Vec::new()),
        span,
    });
    build_image_from_pixels(w, h, &px, span)
}

fn build_positional_form(
    data: &std::cell::Ref<Vec<Value>>,
    start: usize,
    span: Span,
    env: &mut Env,
) -> Result<Value, EvalError> {
    // Collect up to 3 leading expressions: width, height, pixels. Extra
    // trailing values are an error.
    let mut vals: Vec<Value> = Vec::with_capacity(3);
    let mut i = start;
    while i < data.len() && vals.len() < 3 {
        let v = eval_expression(data, &mut i, env)?;
        vals.push(v);
    }
    if i < data.len() {
        return Err(EvalError::Native {
            message: "make image!: expected 2 or 3 elements (width height [pixels]), got more"
                .into(),
            span,
        });
    }
    match vals.len() {
        0 => Ok(Value::image(ImageDef::empty(0, 0))),
        2 => {
            // `[w h]` with no pixels — empty image of size w×h.
            let w = expect_int(&vals[0], "width", span)?.max(0) as usize;
            let h = expect_int(&vals[1], "height", span)?.max(0) as usize;
            Ok(Value::image(ImageDef::empty(w, h)))
        }
        3 => {
            let w = expect_int(&vals[0], "width", span)?.max(0) as usize;
            let h = expect_int(&vals[1], "height", span)?.max(0) as usize;
            build_image_from_pixels(w, h, &vals[2], span)
        }
        n => Err(EvalError::Native {
            message: format!(
                "make image!: expected 2 or 3 elements (width height [pixels]), got {n}"
            ),
            span,
        }),
    }
}

fn build_image_from_pixels(
    width: usize,
    height: usize,
    pixels: &Value,
    span: Span,
) -> Result<Value, EvalError> {
    let bytes = match pixels {
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let mut out = Vec::with_capacity(data.len());
            for v in data.iter() {
                match v {
                    Value::Integer { n, .. } => out.push((*n).clamp(0, 255) as u8),
                    _ => {
                        return Err(EvalError::Native {
                            message: format!(
                                "make image!: pixels block must contain only integers, found {}",
                                type_name(v)
                            ),
                            span: v.span_or_default(),
                        })
                    }
                }
            }
            out
        }
        Value::String8 { bytes, .. } => bytes.clone(),
        _ => {
            return Err(EvalError::TypeError {
                expected: "block! or binary! for pixels",
                found: type_name(pixels),
                span: pixels.span_or_default(),
            })
        }
    };
    ImageDef::from_bytes(width, height, &bytes)
        .map(Value::image)
        .map_err(|m| EvalError::Native { message: m, span })
}

fn expect_int(v: &Value, field: &str, span: Span) -> Result<i64, EvalError> {
    match v {
        Value::Integer { n, .. } => Ok(*n),
        _ => Err(EvalError::Native {
            message: format!(
                "make image!: {field} must be an integer, got {}",
                type_name(v)
            ),
            span,
        }),
    }
}

/// `to-image value` — convert to an `image!`. Same shape as `make image!`
/// minus the identity case (an `image!` arg still copies).
pub(crate) fn to_image(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    let spec = args.first().ok_or_else(|| EvalError::Arity {
        native: Symbol::new("to-image"),
        expected: 1,
        got: 0,
        span: Span::default(),
    })?;
    make_image(spec, env)
}

// ---------------------------------------------------------------------------
// Natives
// ---------------------------------------------------------------------------

/// `image? value` — `true` if value is an `image!`.
fn image_predicate(args: &[Value], _refs: &RefineArgs, _env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(arity_err(args, "image?", 1, 0));
    }
    Ok(Value::Logic(matches!(args[0], Value::Image(_))))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

pub fn register_image_natives(env: &mut Env) {
    use red_core::value::FuncDef;

    let reg = |env: &mut Env, name: &str, f: NF, arity: usize| {
        let params: Vec<Symbol> = (0..arity)
            .map(|i| Symbol::new(&format!("__arg{i}")))
            .collect();
        env.natives.insert(
            Symbol::new(name),
            Rc::new(FuncDef {
                params,
                native: Some(f),
                variadic: false,
                infix: false,
                ..Default::default()
            }),
        );
    };

    reg(env, "image?", image_predicate as NF, 1);
    reg(env, "to-image", to_image as NF, 1);
}

#[cfg(test)]
mod tests {
    //! M85 inline coverage: construct / predicate / pick / poke / paths /
    //! equality. Mirrors the `vector.rs` test layout.

    use super::*;
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives};
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    struct BufferWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for BufferWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
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
        let block = Value::block(body);
        let val = crate::interp::eval(&block, &mut env).map_err(|e| e.to_string())?;
        let out = buf.borrow().clone();
        Ok((val, out))
    }

    fn val(src: &str) -> Value {
        run_capture_val(src).unwrap().0
    }

    fn img_2x1() -> Value {
        // 2×1 image: red pixel, green pixel.
        Value::image(ImageDef::from_bytes(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]).unwrap())
    }

    #[test]
    fn make_image_keyword_form_molds_back() {
        assert_eq!(
            mold_to_string(&img_2x1()),
            "make image! [width: 2 height: 1 pixels: [255 0 0 255 0 255 0 255]]"
        );
    }

    #[test]
    fn make_image_positional_form_molds_back() {
        let im =
            Value::image(ImageDef::from_bytes(1, 2, &[10, 20, 30, 40, 50, 60, 70, 80]).unwrap());
        assert_eq!(
            mold_to_string(&im),
            "make image! [width: 1 height: 2 pixels: [10 20 30 40 50 60 70 80]]"
        );
    }

    #[test]
    fn make_image_empty_block() {
        let v = val("make image! []");
        if let Value::Image(im) = &v {
            assert_eq!(im.borrow().width, 0);
            assert_eq!(im.borrow().height, 0);
            assert_eq!(im.borrow().len(), 0);
        } else {
            panic!("expected image!, got {}", type_name(&v));
        }
    }

    #[test]
    fn make_image_keyword_eval() {
        let v = val("make image! [width: 2 height: 1 pixels: [255 0 0 255 0 255 0 255]]");
        if let Value::Image(im) = &v {
            assert_eq!(im.borrow().width, 2);
            assert_eq!(im.borrow().height, 1);
            assert_eq!(im.borrow().len(), 2);
        } else {
            panic!("expected image!, got {}", type_name(&v));
        }
    }

    #[test]
    fn make_image_positional_eval() {
        let v = val("make image! [2 1 [255 0 0 255 0 255 0 255]]");
        if let Value::Image(im) = &v {
            assert_eq!(im.borrow().width, 2);
            assert_eq!(im.borrow().height, 1);
            assert_eq!(im.borrow().len(), 2);
        } else {
            panic!("expected image!, got {}", type_name(&v));
        }
    }

    #[test]
    fn make_image_binary_pixels() {
        let v = val("make image! [width: 1 height: 1 pixels: #{FF0000FF}]");
        if let Value::Image(im) = &v {
            let p = im.borrow().pixels.borrow().clone();
            assert_eq!(p.as_slice(), &[[255, 0, 0, 255]]);
        } else {
            panic!("expected image!, got {}", type_name(&v));
        }
    }

    #[test]
    fn make_image_bad_dim_errors() {
        let v = run_capture_val("make image! [width: 2 height: 1 pixels: [255 0 0 255]]");
        match v {
            Err(_) => {}
            Ok((other, _)) => panic!("expected error, got {}", type_name(&other)),
        }
    }

    #[test]
    fn image_predicate_true_false() {
        assert!(matches!(
            val("image? make image! [1 1 [0 0 0 0]]"),
            Value::Logic(true)
        ));
        assert!(matches!(val("image? 5"), Value::Logic(false)));
        assert!(matches!(val("image? \"hi\""), Value::Logic(false)));
    }

    #[test]
    fn image_length_is_pixel_count() {
        let v = val("length? make image! [2 3 [0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0]]");
        assert!(matches!(v, Value::Integer { n: 6, .. }));
    }

    #[test]
    fn image_pick_flat_index() {
        let im = img_2x1();
        if let Value::Image(i) = &im {
            let v = i.borrow().pick(1).unwrap();
            assert!(
                matches!(v, Value::Tuple { ref bytes, .. } if bytes.as_ref() == [255, 0, 0, 255])
            );
            let v = i.borrow().pick(2).unwrap();
            assert!(
                matches!(v, Value::Tuple { ref bytes, .. } if bytes.as_ref() == [0, 255, 0, 255])
            );
            assert!(i.borrow().pick(3).is_none());
        } else {
            panic!("expected image!");
        }
    }

    #[test]
    fn image_poke_round_trip() {
        let im = img_2x1();
        if let Value::Image(i) = &im {
            i.borrow()
                .poke(1, &Value::tuple(vec![1, 2, 3, 4]))
                .unwrap()
                .unwrap();
            let p = i.borrow().pixels.borrow().clone();
            assert_eq!(p[0], [1, 2, 3, 4]);
        }
    }

    #[test]
    fn image_poke_3byte_tuple_forces_opaque() {
        let im = img_2x1();
        if let Value::Image(i) = &im {
            i.borrow()
                .poke(1, &Value::tuple(vec![1, 2, 3]))
                .unwrap()
                .unwrap();
            let p = i.borrow().pixels.borrow().clone();
            assert_eq!(p[0], [1, 2, 3, 255]);
        }
    }

    #[test]
    fn image_xy_pick() {
        let im = img_2x1();
        if let Value::Image(i) = &im {
            let v = i.borrow().pick_xy(1, 1).unwrap();
            assert!(
                matches!(v, Value::Tuple { ref bytes, .. } if bytes.as_ref() == [255, 0, 0, 255])
            );
            let v = i.borrow().pick_xy(2, 1).unwrap();
            assert!(
                matches!(v, Value::Tuple { ref bytes, .. } if bytes.as_ref() == [0, 255, 0, 255])
            );
            assert!(i.borrow().pick_xy(3, 1).is_none());
            assert!(i.borrow().pick_xy(0, 1).is_none());
        }
    }

    #[test]
    fn image_xy_poke() {
        let im = img_2x1();
        if let Value::Image(i) = &im {
            i.borrow()
                .poke_xy(2, 1, &Value::tuple(vec![9, 9, 9, 9]))
                .unwrap()
                .unwrap();
            let v = i.borrow().pick_xy(2, 1).unwrap();
            assert!(matches!(v, Value::Tuple { ref bytes, .. } if bytes.as_ref() == [9, 9, 9, 9]));
        }
    }

    #[test]
    fn image_path_width_height() {
        assert!(matches!(
            val("img: make image! [3 2 [0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0]] img/width"),
            Value::Integer { n: 3, .. }
        ));
        assert!(matches!(
            val("img: make image! [3 2 [0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0]] img/height"),
            Value::Integer { n: 2, .. }
        ));
    }

    #[test]
    fn image_path_size_returns_pair() {
        let v = val(
            "img: make image! [3 2 [0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0]] img/size",
        );
        match v {
            Value::Pair { x, y, .. } => {
                assert!(matches!(*x, Value::Integer { n: 3, .. }));
                assert!(matches!(*y, Value::Integer { n: 2, .. }));
            }
            other => panic!("expected pair!, got {}", type_name(&other)),
        }
    }

    #[test]
    fn image_path_flat_pick() {
        assert!(matches!(
            val("img: make image! [2 1 [255 0 0 255 0 255 0 255]] img/1"),
            Value::Tuple { ref bytes, .. } if bytes.as_ref() == [255, 0, 0, 255]
        ));
        assert!(matches!(
            val("img: make image! [2 1 [255 0 0 255 0 255 0 255]] img/2"),
            Value::Tuple { ref bytes, .. } if bytes.as_ref() == [0, 255, 0, 255]
        ));
    }

    #[test]
    fn image_path_xy_pick() {
        assert!(matches!(
            val("img: make image! [2 1 [255 0 0 255 0 255 0 255]] img/1x1"),
            Value::Tuple { ref bytes, .. } if bytes.as_ref() == [255, 0, 0, 255]
        ));
        assert!(matches!(
            val("img: make image! [2 1 [255 0 0 255 0 255 0 255]] img/2x1"),
            Value::Tuple { ref bytes, .. } if bytes.as_ref() == [0, 255, 0, 255]
        ));
        assert!(matches!(
            val("img: make image! [2 1 [255 0 0 255 0 255 0 255]] img/3x1"),
            Value::None
        ));
    }

    #[test]
    fn image_path_set_poke() {
        let v = val("img: make image! [1 1 [0 0 0 0]] img/1: 255.0.0.255 img/1");
        assert!(matches!(v, Value::Tuple { ref bytes, .. } if bytes.as_ref() == [255, 0, 0, 255]));
    }

    #[test]
    fn image_path_set_xy_poke() {
        // Pair set-path (`img/2x1:`) isn't supported by the lexer (only
        // `word:` and `digit:` set-path tails are). Use the `poke` native
        // with a flat 1-based index, then verify via the Pair get-path
        // (`img/2x1` — which DOES work via `Word("/") + Pair` folding).
        let v = val("img: make image! [2 1 [0 0 0 0 0 0 0 0]] poke img 2 255.0.0.255 img/2x1");
        assert!(matches!(v, Value::Tuple { ref bytes, .. } if bytes.as_ref() == [255, 0, 0, 255]));
    }

    #[test]
    fn image_equality_deep() {
        let a = img_2x1();
        let b = img_2x1();
        assert!(crate::natives::values_equal(&a, &b));
        // Different width → unequal.
        let c =
            Value::image(ImageDef::from_bytes(1, 2, &[255, 0, 0, 255, 0, 255, 0, 255]).unwrap());
        assert!(!crate::natives::values_equal(&a, &c));
    }

    #[test]
    fn image_same_identity() {
        assert_eq!(
            mold_to_string(&val("v: make image! [1 1 [0 0 0 0]] same? v v")),
            "true"
        );
        assert_eq!(
            mold_to_string(&val(
                "same? (make image! [1 1 [0 0 0 0]]) (make image! [1 1 [0 0 0 0]])"
            )),
            "false"
        );
    }

    #[test]
    fn image_is_not_series() {
        assert!(matches!(
            val("series? make image! [1 1 [0 0 0 0]]"),
            Value::Logic(false)
        ));
    }
}
