//! M160–M161: Query dialect — `query [...]`.
//!
//! A block-walking dialect for querying blocks of objects (or key/value-pair
//! blocks) with `from`/`select`/`where`/`order`/`limit`/`offset`/`distinct`
//! clauses. Returns a `block!` of matching records (objects by default).
//!
//! Grammar:
//!   query [
//!       from <word-or-block>
//!       select [field1 field2 ...]  ; or `select *` for all fields
//!       where [condition-block]      ; evaluated per-row with fields in scope
//!       order [field1 field2 desc]  ; sort by field(s), ascending by default
//!       limit <integer>
//!       offset <integer>
//!       distinct                    ; remove duplicate rows
//!   ]

use red_core::value::{ObjectDef, Series, Span, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

use crate::interp::dispatch_block;
use crate::natives::{reg_refined, truthy, type_name};
use crate::series::word_sym;
use crate::NativeFn;

// ===========================================================================
// query native
// ===========================================================================

/// `query block` — runs a query against a block of records.
pub fn query_native(args: &[Value], _refs: &RefineArgs, env: &mut Env) -> Result<Value, EvalError> {
    if args.is_empty() {
        return Err(EvalError::Arity {
            native: Symbol::new("query"),
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

    // Parse the query clauses from the rule block.
    let plan = parse_query(&series, env, span)?;

    // Execute the query.
    let result = execute_query(plan, env, span)?;

    Ok(result)
}

// ===========================================================================
// Query plan
// ===========================================================================

struct QueryPlan {
    /// The data source — a resolved block of records.
    source: Vec<Value>,
    /// Projection: `None` = all fields (`select *`), `Some(words)` = selected fields.
    select: Option<Vec<Symbol>>,
    /// WHERE clause block (unevaluated) — evaluated per-row.
    where_block: Option<Series>,
    /// ORDER BY: list of (field, descending) pairs.
    order: Vec<(Symbol, bool)>,
    /// LIMIT: take first N rows.
    limit: Option<usize>,
    /// OFFSET: skip first N rows.
    offset: usize,
    /// DISTINCT: remove duplicate rows.
    distinct: bool,
}

/// Parse the query rule block into a `QueryPlan`. Walks the block left-to-right
/// dispatching on keyword words.
fn parse_query(series: &Series, env: &mut Env, span: Span) -> Result<QueryPlan, EvalError> {
    let data = series.data.borrow();
    let elems: Vec<Value> = data.iter().skip(series.index).cloned().collect();
    drop(data);

    let mut source: Vec<Value> = Vec::new();
    let mut has_from = false;
    let mut select: Option<Vec<Symbol>> = None;
    let mut where_block: Option<Series> = None;
    let mut order: Vec<(Symbol, bool)> = Vec::new();
    let mut limit: Option<usize> = None;
    let mut offset: usize = 0;
    let mut distinct = false;

    let mut i = 0;
    while i < elems.len() {
        let keyword = match word_sym(&elems[i]) {
            Some(s) => s.clone(),
            None => {
                return Err(EvalError::Native {
                    message: format!("query: expected keyword, got {}", type_name(&elems[i])),
                    span: elems[i].span_or_default(),
                });
            }
        };
        i += 1;

        match keyword.as_str() {
            "from" => {
                if i >= elems.len() {
                    return Err(EvalError::Native {
                        message: "query: `from` expects a word or block argument".into(),
                        span,
                    });
                }
                source = resolve_source(&elems[i], env, span)?;
                has_from = true;
                i += 1;
            }
            "select" => {
                if i >= elems.len() {
                    return Err(EvalError::Native {
                        message: "query: `select` expects a block or `*`".into(),
                        span,
                    });
                }
                select = parse_select(&elems[i], span)?;
                i += 1;
            }
            "where" => {
                if i >= elems.len() {
                    return Err(EvalError::Native {
                        message: "query: `where` expects a block".into(),
                        span,
                    });
                }
                match &elems[i] {
                    Value::Block { series, .. } => {
                        where_block = Some(series.clone());
                    }
                    other => {
                        return Err(EvalError::Native {
                            message: format!(
                                "query: `where` expects a block!, got {}",
                                type_name(other)
                            ),
                            span: other.span_or_default(),
                        });
                    }
                }
                i += 1;
            }
            "order" => {
                if i >= elems.len() {
                    return Err(EvalError::Native {
                        message: "query: `order` expects a block of field words".into(),
                        span,
                    });
                }
                order = parse_order(&elems[i], span)?;
                i += 1;
            }
            "limit" => {
                if i >= elems.len() {
                    return Err(EvalError::Native {
                        message: "query: `limit` expects an integer".into(),
                        span,
                    });
                }
                match &elems[i] {
                    Value::Integer { n, .. } => {
                        if *n < 0 {
                            return Err(EvalError::Native {
                                message: format!("query: `limit` must be non-negative, got {n}"),
                                span: elems[i].span_or_default(),
                            });
                        }
                        limit = Some(*n as usize);
                    }
                    other => {
                        return Err(EvalError::Native {
                            message: format!(
                                "query: `limit` expects an integer!, got {}",
                                type_name(other)
                            ),
                            span: other.span_or_default(),
                        });
                    }
                }
                i += 1;
            }
            "offset" => {
                if i >= elems.len() {
                    return Err(EvalError::Native {
                        message: "query: `offset` expects an integer".into(),
                        span,
                    });
                }
                match &elems[i] {
                    Value::Integer { n, .. } => {
                        if *n < 0 {
                            return Err(EvalError::Native {
                                message: format!("query: `offset` must be non-negative, got {n}"),
                                span: elems[i].span_or_default(),
                            });
                        }
                        offset = *n as usize;
                    }
                    other => {
                        return Err(EvalError::Native {
                            message: format!(
                                "query: `offset` expects an integer!, got {}",
                                type_name(other)
                            ),
                            span: other.span_or_default(),
                        });
                    }
                }
                i += 1;
            }
            "distinct" => {
                distinct = true;
            }
            other => {
                return Err(EvalError::Native {
                    message: format!("query: unknown keyword `{other}`"),
                    span,
                });
            }
        }
    }

    if !has_from {
        return Err(EvalError::Native {
            message: "query: no `from` clause found".into(),
            span,
        });
    }

    Ok(QueryPlan {
        source,
        select,
        where_block,
        order,
        limit,
        offset,
        distinct,
    })
}

/// Resolve the data source from a word (looked up in `user_ctx`) or a block
/// (used directly). Returns a flat `Vec<Value>` of records. If the source
/// block contains unevaluated expressions (e.g. `make object! [...]`), they
/// are reduced first.
fn resolve_source(v: &Value, env: &mut Env, span: Span) -> Result<Vec<Value>, EvalError> {
    match v {
        Value::Word { sym, .. }
        | Value::GetWord { sym, .. } => {
            let val = env.user_ctx.get(sym).ok_or_else(|| EvalError::Native {
                message: format!("query: `from` word `{}` has no value", sym.as_str()),
                span,
            })?;
            extract_records(&val, env, span)
        }
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            extract_records(v, env, span)
        }
        Value::LitWord { sym, .. } => {
            let val = env.user_ctx.get(sym).ok_or_else(|| EvalError::Native {
                message: format!("query: `from` word `{}` has no value", sym.as_str()),
                span,
            })?;
            extract_records(&val, env, span)
        }
        other => Err(EvalError::Native {
            message: format!(
                "query: `from` expects a word or block, got {}",
                type_name(other)
            ),
            span: other.span_or_default(),
        }),
    }
}

/// Extract records from a source value. If the source is a block, it's
/// reduced first (so `make object! [...]` calls are evaluated). The result
/// is a flat `Vec<Value>` of records.
fn extract_records(v: &Value, env: &mut Env, span: Span) -> Result<Vec<Value>, EvalError> {
    match v {
        Value::Block { series, .. } | Value::Paren { series, .. } => {
            // Reduce the block to evaluate any `make object!` / other expressions.
            let reduced = crate::interp::dispatch_block_reduce(
                &Value::block(series.clone()),
                env,
            )?;
            match &reduced {
                Value::Block { series: rs, .. } => {
                    let data = rs.data.borrow();
                    Ok(data.iter().skip(rs.index).cloned().collect())
                }
                _ => Ok(vec![reduced]),
            }
        }
        other => Err(EvalError::Native {
            message: format!(
                "query: `from` source must be a block!, got {}",
                type_name(other)
            ),
            span: other.span_or_default(),
        }),
    }
}

/// Parse the `select` argument: a block of field words, or the literal word `*`.
fn parse_select(v: &Value, span: Span) -> Result<Option<Vec<Symbol>>, EvalError> {
    match v {
        Value::Block { series, .. } => {
            let data = series.data.borrow();
            let mut fields = Vec::new();
            for elem in data.iter().skip(series.index) {
                if let Some(sym) = word_sym(elem) {
                    fields.push(sym.clone());
                } else {
                    return Err(EvalError::Native {
                        message: format!(
                            "query: `select` block must contain only words, got {}",
                            type_name(elem)
                        ),
                        span: elem.span_or_default(),
                    });
                }
            }
            Ok(Some(fields))
        }
        Value::Word { sym, .. } | Value::LitWord { sym, .. } if sym.as_str() == "*" => {
            Ok(None) // select * = all fields
        }
        other => Err(EvalError::Native {
            message: format!(
                "query: `select` expects a block or `*`, got {}",
                type_name(other)
            ),
            span: other.span_or_default(),
        }),
    }
}

/// Parse the `order` argument: a block of field words, optionally followed by
/// `asc` or `desc`.
fn parse_order(v: &Value, span: Span) -> Result<Vec<(Symbol, bool)>, EvalError> {
    let series = match v {
        Value::Block { series, .. } | Value::Paren { series, .. } => series.clone(),
        other => {
            return Err(EvalError::Native {
                message: format!("query: `order` expects a block, got {}", type_name(other)),
                span: other.span_or_default(),
            });
        }
    };
    let data = series.data.borrow();
    let elems: Vec<Value> = data.iter().skip(series.index).cloned().collect();
    drop(data);

    let mut result = Vec::new();
    let mut i = 0;
    while i < elems.len() {
        let field = match word_sym(&elems[i]) {
            Some(s) => s.clone(),
            None => {
                return Err(EvalError::Native {
                    message: format!(
                        "query: `order` block must contain words, got {}",
                        type_name(&elems[i])
                    ),
                    span: elems[i].span_or_default(),
                });
            }
        };
        i += 1;

        // Check for `asc`/`desc` modifier.
        let mut desc = false;
        if i < elems.len() {
            if let Some(modifier) = word_sym(&elems[i]) {
                match modifier.as_str() {
                    "desc" => {
                        desc = true;
                        i += 1;
                    }
                    "asc" => {
                        i += 1;
                    }
                    _ => {}
                }
            }
        }
        result.push((field, desc));
    }
    Ok(result)
}

// ===========================================================================
// Query execution
// ===========================================================================

fn execute_query(plan: QueryPlan, env: &mut Env, span: Span) -> Result<Value, EvalError> {
    let mut rows = plan.source;

    // WHERE: filter rows.
    if let Some(where_series) = &plan.where_block {
        rows = filter_rows(rows, where_series, env, span)?;
    }

    // ORDER BY: sort rows.
    if !plan.order.is_empty() {
        sort_rows(&mut rows, &plan.order, span)?;
    }

    // OFFSET + LIMIT.
    if plan.offset > 0 {
        rows = rows.into_iter().skip(plan.offset).collect();
    }
    if let Some(limit) = plan.limit {
        rows.truncate(limit);
    }

    // SELECT: project fields.
    if let Some(fields) = &plan.select {
        rows = project_rows(rows, fields, span)?;
    }

    // DISTINCT: remove duplicates (after projection so projected rows are
    // compared, not full records).
    if plan.distinct {
        rows = dedup_rows(rows);
    }

    Ok(Value::block(Series::new(rows)))
}

/// Filter rows by evaluating the WHERE block per-row. The row's fields are
/// temporarily written into `env.user_ctx` (so Unbound words in the WHERE
/// block resolve to the row's values), then restored after evaluation.
fn filter_rows(
    rows: Vec<Value>,
    where_series: &Series,
    env: &mut Env,
    span: Span,
) -> Result<Vec<Value>, EvalError> {
    let where_block = Value::block(where_series.clone());
    let mut result = Vec::new();
    for row in rows {
        let fields = extract_fields(&row, span)?;

        // Save old values and write row fields into user_ctx so Unbound
        // words in the WHERE block resolve to the row's field values.
        let saved: Vec<(Symbol, Option<Value>)> = fields
            .iter()
            .map(|(sym, _)| (sym.clone(), env.user_ctx.get(sym)))
            .collect();
        for (sym, val) in &fields {
            env.user_ctx.set(sym.clone(), val.clone());
        }

        let eval_result = dispatch_block(&where_block, env);

        // Restore old values.
        for (sym, old) in &saved {
            match old {
                Some(v) => env.user_ctx.set(sym.clone(), v.clone()),
                None => env.user_ctx.set(sym.clone(), Value::None),
            }
        }

        match eval_result {
            Ok(v) => {
                if truthy(&v) {
                    result.push(row);
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}

/// Sort rows by the specified fields. Uses a simple stable sort with
/// lexicographic comparison of the field values.
fn sort_rows(rows: &mut Vec<Value>, order: &[(Symbol, bool)], span: Span) -> Result<(), EvalError> {
    let mut err: Option<EvalError> = None;
    rows.sort_by(|a, b| {
        for (field, desc) in order {
            let av = get_field(a, field).unwrap_or(Value::None);
            let bv = get_field(b, field).unwrap_or(Value::None);
            let ord = compare_values(&av, &bv);
            let ord = if *desc { ord.reverse() } else { ord };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    if let Some(e) = err {
        return Err(e);
    }
    let _ = span;
    Ok(())
}

/// Compare two values for ordering. Tries `num_cmp` for numbers, falls back
/// to `values_equal` for equality, and uses a mold-based string comparison
/// for other types.
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    // Try numeric comparison first.
    if let Ok(ord) = crate::natives::num_cmp(a, b) {
        return ord;
    }
    // Fall back to string comparison via form.
    let a_str = red_core::printer::form_to_string(a);
    let b_str = red_core::printer::form_to_string(b);
    a_str.cmp(&b_str)
}

/// Remove duplicate rows (by structural equality).
fn dedup_rows(rows: Vec<Value>) -> Vec<Value> {
    let mut result = Vec::new();
    for row in rows {
        let is_dup = result.iter().any(|r| crate::natives::values_equal(r, &row));
        if !is_dup {
            result.push(row);
        }
    }
    result
}

/// Project selected fields from each row, returning a new block of objects
/// (or key/value blocks for key/value-pair records).
fn project_rows(rows: Vec<Value>, fields: &[Symbol], span: Span) -> Result<Vec<Value>, EvalError> {
    let mut result = Vec::new();
    for row in rows {
        let mut proj = ObjectDef::new();
        for field in fields {
            let val = get_field(&row, field).unwrap_or(Value::None);
            proj.ctx.set(field.clone(), val);
        }
        result.push(Value::object(proj));
    }
    let _ = span;
    Ok(result)
}

// ===========================================================================
// Field extraction
// ===========================================================================

/// Extract all fields from a record as `(Symbol, Value)` pairs. Handles both
/// `object!` records and key/value-pair `block!` records.
fn extract_fields(record: &Value, _span: Span) -> Result<Vec<(Symbol, Value)>, EvalError> {
    match record {
        Value::Object(obj) => {
            let obj = obj.borrow();
            let words = obj.ctx.words();
            Ok(words
                .iter()
                .filter(|w| w.as_str() != "self")
                .filter_map(|w| obj.ctx.get(w).map(|v| (w.clone(), v)))
                .collect())
        }
        Value::Block { series, .. } => {
            // Key/value-pair block: [name "Alice" age 30].
            let data = series.data.borrow();
            let elems: Vec<Value> = data.iter().skip(series.index).cloned().collect();
            drop(data);
            let mut fields = Vec::new();
            let mut i = 0;
            while i + 1 < elems.len() {
                if let Some(sym) = word_sym(&elems[i]) {
                    fields.push((sym.clone(), elems[i + 1].clone()));
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Ok(fields)
        }
        other => Err(EvalError::Native {
            message: format!(
                "query: record must be object! or block!, got {}",
                type_name(other)
            ),
            span: other.span_or_default(),
        }),
    }
}

/// Read a single field from a record by name. Returns `None` if the field
/// doesn't exist.
fn get_field(record: &Value, field: &Symbol) -> Option<Value> {
    match record {
        Value::Object(obj) => obj.borrow().ctx.get(field),
        Value::Block { series, .. } => {
            // Linear scan for the key, return the next value.
            let data = series.data.borrow();
            let mut i = series.index;
            while i + 1 < data.len() {
                if let Some(sym) = word_sym(&data[i]) {
                    if sym == field {
                        return Some(data[i + 1].clone());
                    }
                }
                i += 2;
            }
            None
        }
        _ => None,
    }
}

// ===========================================================================
// Registration
// ===========================================================================

pub fn register_query_natives(env: &mut Env) {
    reg_refined(env, "query", query_native as NativeFn, 1, &[]);
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::{get_field, register_query_natives};
    use crate::binding::bind_pass;
    use crate::natives::{install_constants, register_natives, type_name};
    use crate::eval;
    use crate::json::register_json_natives;
    use crate::html::register_html_natives;
    use red_core::context::Context;
    use red_core::parser::load_source;
    use red_core::printer::mold_to_string;
    use red_core::value::{Symbol, Value};
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
        register_query_natives(&mut env);
        register_json_natives(&mut env);
        register_html_natives(&mut env);
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

    fn m(v: &Value) -> String {
        mold_to_string(v)
    }

    // Helper: set up a people block for tests.
    const PEOPLE_SRC: &str = "
people: [
    make object! [name: \"Alice\" age: 30 city: \"NYC\"]
    make object! [name: \"Bob\" age: 25 city: \"LA\"]
    make object! [name: \"Carol\" age: 41 city: \"NYC\"]
]
";

    #[test]
    fn query_select_all() {
        let v = val(&format!("{PEOPLE_SRC} query [from people]"));
        let block = match &v {
            Value::Block { series, .. } => series.data.borrow().len() - series.index,
            _ => panic!("expected block"),
        };
        assert_eq!(block, 3, "should return all 3 records");
    }

    #[test]
    fn query_select_fields() {
        let v = val(&format!("{PEOPLE_SRC} query [from people select [name]]"));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                assert_eq!(data.len() - series.index, 3);
                // Each projected record should be an object with only `name`.
                for elem in data.iter().skip(series.index) {
                    match elem {
                        Value::Object(obj) => {
                            let obj = obj.borrow();
                            let words: Vec<String> =
                                obj.ctx.words().iter().map(|w| w.as_str().into()).collect();
                            assert!(words.contains(&"name".into()));
                            assert!(!words.contains(&"age".into()));
                        }
                        other => panic!("expected object, got {}", type_name(other)),
                    }
                }
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_where() {
        let v = val(&format!("{PEOPLE_SRC} query [from people where [age > 30]]"));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let count = data.len() - series.index;
                assert_eq!(count, 1, "only Carol has age > 30");
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_where_multiple_fields() {
        let v = val(&format!(
            "{PEOPLE_SRC} query [from people where [all [age > 20 city = \"NYC\"]]]"
        ));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let count = data.len() - series.index;
                assert_eq!(count, 2, "Alice and Carol are in NYC with age > 20");
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_order_ascending() {
        let v = val(&format!("{PEOPLE_SRC} query [from people order [age]]"));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let ages: Vec<i64> = data
                    .iter()
                    .skip(series.index)
                    .map(|r| match get_field(r, &Symbol::new("age")) {
                        Some(Value::Integer { n, .. }) => n,
                        _ => 0,
                    })
                    .collect();
                assert_eq!(ages, vec![25, 30, 41], "ascending by age");
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_order_descending() {
        let v = val(&format!("{PEOPLE_SRC} query [from people order [age desc]]"));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let ages: Vec<i64> = data
                    .iter()
                    .skip(series.index)
                    .map(|r| match get_field(r, &Symbol::new("age")) {
                        Some(Value::Integer { n, .. }) => n,
                        _ => 0,
                    })
                    .collect();
                assert_eq!(ages, vec![41, 30, 25], "descending by age");
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_limit_offset() {
        let v = val(&format!("{PEOPLE_SRC} query [from people order [age] limit 2 offset 1]"));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let count = data.len() - series.index;
                assert_eq!(count, 2, "limit 2 offset 1 → 2 rows");
                let ages: Vec<i64> = data
                    .iter()
                    .skip(series.index)
                    .map(|r| match get_field(r, &Symbol::new("age")) {
                        Some(Value::Integer { n, .. }) => n,
                        _ => 0,
                    })
                    .collect();
                // Sorted by age: [25, 30, 41]. Skip 1 → [30, 41]. Limit 2 → [30, 41].
                assert_eq!(ages, vec![30, 41]);
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_distinct() {
        let v = val(&format!(
            "{PEOPLE_SRC} query [from people select [city] distinct]"
        ));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let count = data.len() - series.index;
                assert_eq!(count, 2, "2 distinct cities: NYC, LA");
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_chained() {
        let v = val(&format!(
            "{PEOPLE_SRC} query [from people where [age > 20] order [age desc] select [name age] limit 2]"
        ));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let count = data.len() - series.index;
                assert_eq!(count, 2, "limit 2");
                let ages: Vec<i64> = data
                    .iter()
                    .skip(series.index)
                    .map(|r| match get_field(r, &Symbol::new("age")) {
                        Some(Value::Integer { n, .. }) => n,
                        _ => 0,
                    })
                    .collect();
                assert_eq!(ages, vec![41, 30], "desc by age, top 2");
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_on_keyvalue_blocks() {
        let v = val("
            data: [[name \"Alice\" age 30] [name \"Bob\" age 25]]
            query [from data where [age > 26] select [name]]
        ");
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                let count = data.len() - series.index;
                assert_eq!(count, 1, "only Alice has age > 26");
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_select_star() {
        let v = val(&format!("{PEOPLE_SRC} query [from people select *]"));
        match &v {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                assert_eq!(data.len() - series.index, 3, "select * returns all rows");
            }
            other => panic!("expected block, got {}", type_name(other)),
        }
    }

    #[test]
    fn query_error_no_from() {
        let result = run_capture_val("query [select [name]]");
        assert!(result.is_err(), "query without from should error");
    }
}
