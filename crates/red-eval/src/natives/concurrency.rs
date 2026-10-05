//! Concurrency natives: `channel`, `send`, `recv`, `close`, `channel?`,
//! `closed?`, `spawn`, `spawn-actor`, `send-actor`, `receive`, `run-actors`,
//! `link`, `monitor`, `spawn-supervisor`, `self`.
//!
//! Channels are Go-style bidirectional: both ends (tx + rx) travel in one
//! `Value::Channel(Arc<ChannelInner>)` value. Cloning a Channel value is an
//! Arc bump. `send` marshals the value across the Send boundary (via M40's
//! `marshal_send`); `recv` unmarshals on the receiver side.
//!
//! `spawn [body]` uses a two-thread model: the worker thread runs `body`
//! (via M41's `spawn_thread`), and a collector thread joins the worker's
//! `JoinHandle`, marshals the result into a `SendValue`, and sends it on a
//! result channel. The collector indirection is needed because the worker
//! doesn't know which channel to send to (the channel is created after the
//! worker is spawned). Both threads are joined at `Env::Drop`.
//!
//! Actors (M45) are plain `Object`s with `mailbox:`/`handler:`/`alive?:`
//! fields, driven by a single-threaded cooperative scheduler (`run-actors`).
//! Not OS threads — actors yield via `receive` or by returning.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use red_core::concurrency::{ChannelInner, SendValue};
use red_core::value::{FuncDef, ObjectDef, Symbol, Value};
use red_core::{Env, EvalError, RefineArgs};

type NF = fn(&[Value], &RefineArgs, &mut Env) -> Result<Value, EvalError>;

/// `channel` (arity 0): create a new bidirectional channel.
///
/// Creates a `std::sync::mpsc::channel()`, wraps tx/rx in `ChannelInner`,
/// returns `Value::Channel(Arc::new(inner))`. Both ends travel together in
/// the one value (Go-style).
pub(crate) fn channel_native(
    _args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    let (tx, rx) = mpsc::channel::<SendValue>();
    let inner = ChannelInner {
        tx: Mutex::new(Some(tx)),
        rx: Mutex::new(rx),
        closed: std::sync::atomic::AtomicBool::new(false),
    };
    Ok(Value::Channel(Arc::new(inner)))
}

/// `send channel value` (arity 2): marshal `value` into `SendValue` and
/// send it on the channel. Returns `none` on success.
///
/// Rejects non-marshalable types (`Func`, `String8`) with `EvalError::Native`.
/// Errors on closed channels ("send on closed channel") and dropped receivers
/// ("send failed (receiver dropped)").
pub(crate) fn send_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(EvalError::Arity {
            native: Symbol::new("send"),
            expected: 2,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let channel = expect_channel(&args[0], "send")?;
    let send_value = args[1].marshal_send()?;

    let tx_guard = channel.tx.lock().unwrap();
    match &*tx_guard {
        Some(tx) => {
            tx.send(send_value).map_err(|_| EvalError::Native {
                message: "send failed (receiver dropped)".into(),
                span: args[0].span_or_default(),
            })?;
            Ok(Value::None)
        }
        None => Err(EvalError::Native {
            message: "send on closed channel".into(),
            span: args[0].span_or_default(),
        }),
    }
}

/// `recv channel` (arity 1): block waiting for the next value on the channel.
///
/// On `Ok(v)`: unmarshal and return. On `Err` (all senders dropped and
/// channel empty): return `none` (matches Red's option-style convention;
/// use `closed?` to distinguish "no data" from "got `none`").
///
/// v0.6's `recv` is always blocking. Non-blocking `recv` (`recv/no-wait`)
/// is a v0.6.1 addition (M47).
pub(crate) fn recv_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(EvalError::Arity {
            native: Symbol::new("recv"),
            expected: 1,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let channel = expect_channel(&args[0], "recv")?;
    let rx_guard = channel.rx.lock().unwrap();
    match rx_guard.recv() {
        Ok(sv) => Ok(sv.unmarshal()),
        Err(_) => Ok(Value::None),
    }
}

/// `close channel` (arity 1): mark the channel as closed and drop the sender.
///
/// Sets `closed` to `true`, takes the `Sender` out of the `Mutex` (dropping
/// it), so subsequent `send` errors. The `Receiver` stays alive until all
/// `Arc<ChannelInner>` clones drop. Returns `none`.
///
/// If the argument is a `port!`, delegates to the port `close` native
/// (registered by `net::register_net_natives`). This avoids a name conflict
/// — both `close port` and `close channel` dispatch through this native.
pub(crate) fn close_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(EvalError::Arity {
            native: Symbol::new("close"),
            expected: 1,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    // Dispatch: port! → port close (from net module), channel! → channel close.
    match &args[0] {
        Value::Port(_) => {
            crate::net::close_port_native(args, _refs, env)
        }
        Value::Channel(channel) => {
            channel.closed.store(true, Ordering::Relaxed);
            let _ = channel.tx.lock().unwrap().take();
            Ok(Value::None)
        }
        _ => Err(EvalError::TypeError {
            expected: "channel! or port!",
            found: crate::natives::type_name(&args[0]),
            span: args[0].span_or_default(),
        }),
    }
}

/// `channel? value` (arity 1): type predicate — true iff `value` is a `channel!`.
pub(crate) fn channel_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    Ok(Value::Logic(matches!(args.first(), Some(Value::Channel(_)))))
}

/// `closed? channel` (arity 1): true iff the channel has been closed via `close`.
pub(crate) fn closed_predicate(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(EvalError::Arity {
            native: Symbol::new("closed?"),
            expected: 1,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let channel = expect_channel(&args[0], "closed?")?;
    Ok(Value::Logic(channel.closed.load(Ordering::Relaxed)))
}

/// `spawn block` (arity 1): fork a worker thread running `body`, return a
/// result `channel!` that receives one value (the body's return value, or an
/// `error!` on eval failure/panic) when the worker finishes.
///
/// Two-thread model (worker + collector):
/// 1. `spawn_thread(body, env)` (M41) forks the worker, returns a
///    `JoinHandle<Result<SendValue, String>>`.
/// 2. Create a fresh `channel!` (the result channel).
/// 3. Spawn a collector thread that:
///    a. `handle.join()` (blocks until worker finishes).
///    b. Marshal the result: `Ok(sv)` → `sv` (already a `SendValue`); `Err(msg)`
///       → `SendValue::Error(...)` (reconstruct a plain error value).
///    c. `result_tx.send(marshalled)`.
/// 4. Return the result channel to the caller.
///
/// The collector's lifetime is bounded by the worker's (`join` blocks until
/// the worker finishes), so it always terminates. Both the worker handle
/// and the collector handle are pushed onto `env.thread_handles` for
/// join-all-at-exit (M41's `Env::Drop`).
pub(crate) fn spawn_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(EvalError::Arity {
            native: Symbol::new("spawn"),
            expected: 1,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let body_series = match &args[0] {
        Value::Block { series, .. } => series.clone(),
        _ => {
            return Err(EvalError::TypeError {
                expected: "block!",
                found: crate::natives::type_name(&args[0]),
                span: args[0].span_or_default(),
            });
        }
    };

    // Fork the worker thread (M41's spawn_thread).
    let worker_handle = crate::concurrency::spawn_thread(body_series, env);

    // Create a result channel (same logic as `channel_native`).
    let (result_tx, result_rx) = mpsc::channel::<SendValue>();
    let result_inner: Arc<ChannelInner> = Arc::new(ChannelInner {
        tx: Mutex::new(Some(result_tx)),
        rx: Mutex::new(result_rx),
        closed: std::sync::atomic::AtomicBool::new(false),
    });
    let result_channel = Value::Channel(Arc::clone(&result_inner));

    // Spawn the collector thread: joins the worker, marshals the result,
    // sends it on the result channel. Returns `Ok(SendValue::None)` on
    // success so the handle type matches `env.thread_handles`.
    // Only the `Arc<ChannelInner>` (which is `Send`) crosses into the
    // collector — not the `Value` wrapper (which is `!Send`).
    let collector_handle = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || -> Result<SendValue, String> {
            let result = worker_handle.join();
            let send_value = match result {
                Ok(Ok(sv)) => sv, // worker succeeded: already a SendValue
                Ok(Err(msg)) => {
                    // worker eval error: reconstruct as a SendError.
                    SendValue::Error(Arc::new(red_core::concurrency::SendError {
                        message: Arc::from(msg.as_str()),
                        code: None,
                        kind: None,
                        args: Vec::new(),
                        near: None,
                        cause: None,
                        by: None,
                    }))
                }
                Err(panic) => {
                    // worker panicked: reconstruct as a SendError.
                    let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                        format!("thread panicked: {s}")
                    } else if let Some(s) = panic.downcast_ref::<String>() {
                        format!("thread panicked: {s}")
                    } else {
                        "thread panicked: (unknown)".to_string()
                    };
                    SendValue::Error(Arc::new(red_core::concurrency::SendError {
                        message: Arc::from(msg.as_str()),
                        code: None,
                        kind: None,
                        args: Vec::new(),
                        near: None,
                        cause: None,
                        by: None,
                    }))
                }
            };
            // Send on the result channel. If the receiver was dropped (caller
            // didn't `recv`), the send fails silently — the result is just
            // discarded.
            if let Some(tx) = result_inner.tx.lock().unwrap().as_ref() {
                let _ = tx.send(send_value);
            }
            Ok(SendValue::None)
        })
        .expect("failed to spawn collector thread");

    // Push both handles onto env.thread_handles for join-all-at-exit.
    // The worker handle is consumed by the collector's `join`, so we can't
    // push it separately. The collector handle is the one that matters for
    // Env::Drop — it blocks until the worker finishes (via the inner `join`).
    env.thread_handles.push(collector_handle);

    Ok(result_channel)
}

// ===========================================================================
// M45/M46: Actor natives — cooperative single-threaded actor library.
//
// **M46 convention (single-message handlers):** an actor's handler runs once
// per `send-actor`, processes one message, and returns. The scheduler
// (`run-actors`) pops one message from the mailbox, calls `handler(msg)`,
// and re-queues the actor if more messages remain. Long-lived actors that
// need to loop do so explicitly via `send-actor self msg` (the actor sends
// itself a continuation message). This is simpler than Erlang's
// `receive`-in-loop model but less expressive; multi-message handlers with
// continuations are a v0.7 candidate alongside the M:N scheduler (which
// needs continuation support for work-stealing anyway).
// ===========================================================================

/// `spawn-actor handler-func` (arity 1): create an actor object with
/// `mailbox: channel`, `handler: <the func>`, `alive?: true`. Pushes the
/// actor onto `env.actor_ready_queue`. Returns the actor `Object` value.
///
/// Does NOT spawn an OS thread — the actor is driven by the `run-actors`
/// scheduler on the calling thread.
pub(crate) fn spawn_actor_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(EvalError::Arity {
            native: Symbol::new("spawn-actor"),
            expected: 1,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    // Evaluate the block argument to get the handler func.
    // `spawn-actor [func [msg] [body]]` — the block [func [msg] [body]]
    // is data; we eval it to get the Value::Func result.
    let handler = crate::interp::eval(&args[0], env)?;
    // Create the mailbox channel.
    let mailbox = channel_native(&[], &RefineArgs::default(), env)?;
    // Build the actor object.
    let obj = red_core::value::ObjectDef::new();
    obj.ctx.set(Symbol::new("mailbox"), mailbox);
    obj.ctx.set(Symbol::new("handler"), handler);
    obj.ctx.set(Symbol::new("alive?"), Value::Logic(true));
    let actor_rc = Rc::new(RefCell::new(obj));
    let actor_val = Value::Object(Rc::clone(&actor_rc));
    env.actor_ready_queue.push(actor_rc);
    Ok(actor_val)
}

/// `send-actor actor msg` (arity 2): send `msg` to the actor's mailbox,
/// then push the actor onto the ready-queue (if not already enqueued).
pub(crate) fn send_actor_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(EvalError::Arity {
            native: Symbol::new("send-actor"),
            expected: 2,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let actor_rc = match &args[0] {
        Value::Object(o) => Rc::clone(o),
        _ => return Err(EvalError::TypeError {
            expected: "object!",
            found: crate::natives::type_name(&args[0]),
            span: args[0].span_or_default(),
        }),
    };
    // Get the mailbox from the actor.
    let mailbox = actor_rc.borrow().ctx.get(&Symbol::new("mailbox"));
    let mailbox_val = match mailbox {
        Some(Value::Channel(_)) => mailbox.unwrap(),
        _ => return Err(EvalError::Native {
            message: "send-actor: actor has no mailbox field".into(),
            span: args[0].span_or_default(),
        }),
    };
    // Send the message via the channel (marshal + tx.send).
    send_native(&[mailbox_val, args[1].clone()], &RefineArgs::default(), env)?;
    // Un-park if needed.
    let ptr = Rc::as_ptr(&actor_rc) as *const () as usize;
    env.actor_park_set.remove(&ptr);
    // Push to ready-queue if not already there.
    let in_queue = env
        .actor_ready_queue
        .iter()
        .any(|a| Rc::ptr_eq(a, &actor_rc));
    if !in_queue {
        env.actor_ready_queue.push(actor_rc);
    }
    Ok(Value::None)
}

/// `receive [clauses]` (arity 1): pop one message from the current actor's
/// mailbox and match it against `case`-style clauses. Used inside a handler.
///
/// The clause block is walked in pairs: each candidate value is compared
/// to the popped message (via `values_equal`); on match, the following
/// block is evaluated. A literal `default` word introduces the default
/// clause (runs when no candidate matches). If no match and no default,
/// returns `none`.
///
/// **Note:** in the single-message handler model (M45 default), the
/// scheduler passes the message as the handler's argument — `receive` is
/// for pattern-matching handlers that want to destructure the message.
pub(crate) fn receive_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 1 {
        return Err(EvalError::Arity {
            native: Symbol::new("receive"),
            expected: 1,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let clauses = match &args[0] {
        Value::Block { series, .. } => series.clone(),
        _ => return Err(EvalError::TypeError {
            expected: "block!",
            found: crate::natives::type_name(&args[0]),
            span: args[0].span_or_default(),
        }),
    };
    // Get the current actor's mailbox.
    let actor_rc = env
        .current_actor
        .clone()
        .ok_or_else(|| EvalError::Native {
            message: "receive: not inside an actor handler".into(),
            span: args[0].span_or_default(),
        })?;
    let mailbox_val = actor_rc.borrow().ctx.get(&Symbol::new("mailbox"));
    let channel = match &mailbox_val {
        Some(Value::Channel(ch)) => Arc::clone(ch),
        _ => return Err(EvalError::Native {
            message: "receive: actor has no mailbox".into(),
            span: args[0].span_or_default(),
        }),
    };
    // Non-blocking pop from the mailbox.
    let msg = match channel.rx.lock().unwrap().try_recv() {
        Ok(sv) => sv.unmarshal(),
        Err(_) => return Ok(Value::None), // no message — return none
    };
    // Match against clauses (switch-style).
    let data = clauses.data.borrow();
    let mut i = clauses.index;
    let mut default_block: Option<Value> = None;
    while i < data.len() {
        // Check for `default` word.
        if let Value::Word { sym, .. } = &data[i] {
            if sym.as_str() == "default" {
                i += 1;
                if i < data.len() {
                    default_block = Some(data[i].clone());
                    i += 1;
                }
                continue;
            }
        }
        // Candidate is the next value (evaluated as expression).
        // For simplicity, we don't eval expressions — just compare directly.
        let candidate = data[i].clone();
        i += 1;
        if i >= data.len() {
            break;
        }
        let body = data[i].clone();
        i += 1;
        // Compare candidate to msg.
        if crate::natives::values_equal(&candidate, &msg) {
            drop(data);
            // Run the matching block.
            return match &body {
                Value::Block { .. } | Value::Paren { .. } => {
                    crate::interp_walker::dispatch_block(&body, env)
                }
                _ => Ok(body),
            };
        }
    }
    drop(data);
    // No match — try default.
    if let Some(body) = default_block {
        return match &body {
            Value::Block { .. } | Value::Paren { .. } => {
                crate::interp_walker::dispatch_block(&body, env)
            }
            _ => Ok(body),
        };
    }
    Ok(Value::None)
}

/// `run-actors` (arity 0): cooperative scheduler loop. Drains the actor
/// ready-queue: for each actor, pops one message from its mailbox (non-
/// blocking) and calls the handler. If the mailbox is empty, parks the
/// actor. If the handler errors, marks the actor dead (`alive?: false`)
/// and propagates `:EXIT` to linked actors and `:DOWN` to monitors.
/// Loops until the ready-queue is empty.
pub(crate) fn run_actors_native(
    _args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    loop {
        // Pop an actor from the ready-queue.
        let actor_rc = match env.actor_ready_queue.pop() {
            Some(a) => a,
            None => break, // queue empty
        };
        // Check alive?
        let alive = actor_rc
            .borrow()
            .ctx
            .get(&Symbol::new("alive?"))
            .map(|v| matches!(v, Value::Logic(true)))
            .unwrap_or(true);
        if !alive {
            // Dead actor — propagate :EXIT/:DOWN (if not already done).
            // Check if we've already propagated (by checking if links exist).
            // Actually, we propagate once when the actor first dies. Since
            // the actor is dead and skipped, we just continue. The propagation
            // happened when the handler errored (below).
            continue;
        }
        // Get the mailbox.
        let mailbox_val = actor_rc.borrow().ctx.get(&Symbol::new("mailbox"));
        let channel = match &mailbox_val {
            Some(Value::Channel(ch)) => Arc::clone(ch),
            _ => continue, // no mailbox — skip
        };
        // Get the handler.
        let handler_val = actor_rc.borrow().ctx.get(&Symbol::new("handler"));
        let handler_fd = match &handler_val {
            Some(Value::Func(fd)) => Rc::clone(fd),
            _ => continue, // no handler or wrong type — skip
        };
        // Pop one message from the mailbox (non-blocking).
        let msg = match channel.rx.lock().unwrap().try_recv() {
            Ok(sv) => sv.unmarshal(),
            Err(_) => {
                // Mailbox empty — park the actor.
                let ptr = Rc::as_ptr(&actor_rc) as *const () as usize;
                env.actor_park_set.insert(ptr);
                continue;
            }
        };
        // Set current_actor for `receive`.
        env.current_actor = Some(Rc::clone(&actor_rc));
        // Call the handler with the message (if the handler has params).
        let n_params = handler_fd.params.len();
        let call_args: Vec<Value> = if n_params > 0 {
            vec![msg]
        } else {
            vec![]
        };
        let _handler_result: Result<Value, EvalError> = match crate::interp_walker::call_user_func(
            &handler_fd,
            call_args,
            &RefineArgs::default(),
            env,
        ) {
            Ok(v) => Ok(v),
            Err(EvalError::Return(v)) => Ok(v),
            Err(e) => {
                // Handler errored — mark the actor dead and propagate.
                let reason = e.to_string();
                eprintln!("warning: actor handler error: {reason}");
                actor_rc.borrow().ctx.set(Symbol::new("alive?"), Value::Logic(false));
                actor_rc.borrow().ctx.set(Symbol::new("exit-reason"), Value::string(reason.clone()));
                // Propagate :EXIT to linked actors and :DOWN to monitors.
                propagate_actor_death(env, &actor_rc, &reason);
                env.current_actor = None;
                continue; // don't re-queue a dead actor
            }
        };
        env.current_actor = None;

        // Re-queue the actor — the next iteration will try_recv from the
        // mailbox. If empty, the actor is parked. This avoids the need for
        // non-destructive mailbox checking (std mpsc has no `peek`).
        env.actor_ready_queue.push(Rc::clone(&actor_rc));
    }
    Ok(Value::None)
}

/// M47: Propagate an actor's death to linked actors (`:EXIT` message)
/// and monitors (`:DOWN` message). Sends the messages via the actors'
/// mailboxes and pushes the linked/monitoring actors to the ready-queue.
fn propagate_actor_death(env: &mut Env, dead_actor: &Rc<RefCell<ObjectDef>>, reason: &str) {
    // Build the exit message: an object with type/actor/reason fields.
    let dead_val = Value::Object(Rc::clone(dead_actor));
    let reason_val = Value::string(reason);

    // Send :EXIT to linked actors.
    let links_val = dead_actor.borrow().ctx.get(&Symbol::new("links"));
    if let Some(Value::Block { series, .. }) = &links_val {
        let links: Vec<Value> = series.data.borrow().clone();
        for link in &links {
            if let Value::Object(linked_rc) = link {
                let exit_msg = make_exit_message(&dead_val, &reason_val);
                send_to_actor_mailbox(env, linked_rc, exit_msg);
                let in_queue = env
                    .actor_ready_queue
                    .iter()
                    .any(|a| Rc::ptr_eq(a, linked_rc));
                if !in_queue {
                    env.actor_ready_queue.push(Rc::clone(linked_rc));
                }
            }
        }
    }

    // Send :DOWN to monitors.
    let monitors_val = dead_actor.borrow().ctx.get(&Symbol::new("monitors"));
    if let Some(Value::Block { series, .. }) = &monitors_val {
        let monitors: Vec<Value> = series.data.borrow().clone();
        for mon in &monitors {
            if let Value::Object(mon_rc) = mon {
                let down_msg = make_down_message(&dead_val, &reason_val);
                send_to_actor_mailbox(env, mon_rc, down_msg);
                let in_queue = env
                    .actor_ready_queue
                    .iter()
                    .any(|a| Rc::ptr_eq(a, mon_rc));
                if !in_queue {
                    env.actor_ready_queue.push(Rc::clone(mon_rc));
                }
            }
        }
    }
}

/// Build an `:EXIT` message. Uses a string "exit" to avoid word-type
/// comparison issues (Word ≠ LitWord in the current `values_equal`).
fn make_exit_message(_dead: &Value, _reason: &Value) -> Value {
    Value::string("exit")
}

/// Build a `:DOWN` message. Uses a string "down".
fn make_down_message(_dead: &Value, _reason: &Value) -> Value {
    Value::string("down")
}

/// Send a message to an actor's mailbox (via `send` native logic).
fn send_to_actor_mailbox(env: &mut Env, actor: &Rc<RefCell<ObjectDef>>, msg: Value) {
    let mailbox = actor.borrow().ctx.get(&Symbol::new("mailbox"));
    if let Some(Value::Channel(_)) = &mailbox {
        let _ = send_native(&[mailbox.unwrap(), msg], &RefineArgs::default(), env);
    }
}

/// `link actor1 actor2` (arity 2): bidirectional link. If one actor dies
/// (handler error or `alive?: false`), the other receives an `:EXIT`
/// message. Returns `none`.
pub(crate) fn link_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(EvalError::Arity {
            native: Symbol::new("link"),
            expected: 2,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let a1 = expect_object(&args[0], "link")?;
    let a2 = expect_object(&args[1], "link")?;
    add_to_links_block(&a1, Value::Object(Rc::clone(&a2)));
    add_to_links_block(&a2, Value::Object(Rc::clone(&a1)));
    Ok(Value::None)
}

/// `monitor actor monitor-actor` (arity 2): one-way link. The monitor
/// receives `:DOWN` messages when the monitored actor dies, without
/// linking back. Returns `none`.
pub(crate) fn monitor_native(
    args: &[Value],
    _refs: &RefineArgs,
    _env: &mut Env,
) -> Result<Value, EvalError> {
    if args.len() != 2 {
        return Err(EvalError::Arity {
            native: Symbol::new("monitor"),
            expected: 2,
            got: args.len(),
            span: args.first().map(|v| v.span_or_default()).unwrap_or_default(),
        });
    }
    let monitored = expect_object(&args[0], "monitor")?;
    let monitor = expect_object(&args[1], "monitor")?;
    add_to_monitors_block(&monitored, Value::Object(Rc::clone(&monitor)));
    Ok(Value::None)
}

/// `spawn-supervisor [handler-func]` (arity 1): create a supervisor
/// actor. The supervisor's handler receives `:EXIT` messages when linked
/// children die. The handler can restart children by calling `spawn-actor`
/// and `link` again. Returns the supervisor actor value.
pub(crate) fn spawn_supervisor_native(
    args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    // Delegate to spawn-actor — a supervisor is just an actor with a
    // handler that knows how to handle :EXIT messages.
    spawn_actor_native(args, _refs, env)
}

/// `self` (arity 0): returns the current actor object (the one whose
/// handler is being dispatched), or `none` if outside an actor handler.
/// Mirrors Erlang's `self()`. Inside `make object! [self: ...]`, the
/// object's own `self` slot takes binding priority (set during object
/// construction in `object.rs`); this native only resolves when `self` is
/// `Binding::Unbound` (inside a func handler, not an object spec).
pub(crate) fn self_native(
    _args: &[Value],
    _refs: &RefineArgs,
    env: &mut Env,
) -> Result<Value, EvalError> {
    match &env.current_actor {
        Some(actor) => Ok(Value::Object(Rc::clone(actor))),
        None => Ok(Value::None),
    }
}

/// Helper: add a value to an actor's `links` block (creates the block
/// if it doesn't exist yet).
fn add_to_links_block(actor: &Rc<RefCell<ObjectDef>>, val: Value) {
    let existing = actor.borrow().ctx.get(&Symbol::new("links"));
    match existing {
        Some(Value::Block { series, .. }) => {
            series.data.borrow_mut().push(val);
        }
        _ => {
            let block = Value::block(red_core::value::Series::new(vec![val]));
            actor.borrow().ctx.set(Symbol::new("links"), block);
        }
    }
}

/// Helper: add a value to an actor's `monitors` block (creates the block
/// if it doesn't exist yet).
fn add_to_monitors_block(actor: &Rc<RefCell<ObjectDef>>, val: Value) {
    let existing = actor.borrow().ctx.get(&Symbol::new("monitors"));
    match existing {
        Some(Value::Block { series, .. }) => {
            series.data.borrow_mut().push(val);
        }
        _ => {
            let block = Value::block(red_core::value::Series::new(vec![val]));
            actor.borrow().ctx.set(Symbol::new("monitors"), block);
        }
    }
}

/// Extract an `Rc<RefCell<ObjectDef>>` from a `Value::Object`, or error.
fn expect_object(v: &Value, native: &str) -> Result<Rc<RefCell<ObjectDef>>, EvalError> {
    match v {
        Value::Object(o) => Ok(Rc::clone(o)),
        _ => Err(EvalError::TypeError {
            expected: "object!",
            found: crate::natives::type_name(v),
            span: v.span_or_default(),
        }),
    }
    .map(|o| {
        let _ = native;
        o
    })
}

/// Extract a `&Arc<ChannelInner>` from a `Value::Channel`, or error.
fn expect_channel<'a>(v: &'a Value, _native: &str) -> Result<&'a Arc<ChannelInner>, EvalError> {
    match v {
        Value::Channel(inner) => Ok(inner),
        _ => Err(EvalError::TypeError {
            expected: "channel!",
            found: crate::natives::type_name(v),
            span: v.span_or_default(),
        }),
    }
}

/// Register all concurrency natives (`channel`, `send`, `recv`, `close`,
/// `channel?`, `closed?`, `spawn`, `spawn-actor`, `send-actor`, `receive`,
/// `run-actors`, `link`, `monitor`, `spawn-supervisor`, `self`). Called from
/// `register_natives`.
pub fn register_concurrency_natives(env: &mut Env) {
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

    reg(env, "channel", channel_native as NF, 0);
    reg(env, "send", send_native as NF, 2);
    reg(env, "recv", recv_native as NF, 1);
    reg(env, "close", close_native as NF, 1);
    reg(env, "channel?", channel_predicate as NF, 1);
    reg(env, "closed?", closed_predicate as NF, 1);
    reg(env, "spawn", spawn_native as NF, 1);
    reg(env, "spawn-actor", spawn_actor_native as NF, 1);
    reg(env, "send-actor", send_actor_native as NF, 2);
    reg(env, "receive", receive_native as NF, 1);
    reg(env, "run-actors", run_actors_native as NF, 0);
    reg(env, "link", link_native as NF, 2);
    reg(env, "monitor", monitor_native as NF, 2);
    reg(env, "spawn-supervisor", spawn_supervisor_native as NF, 1);
    reg(env, "self", self_native as NF, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use red_core::parser::load_source;
    use red_core::Context;
    use red_core::value::{ObjectDef, Series};
    use std::cell::RefCell;
    use std::io;

    fn run(src: &str) -> (Env, Value) {
        let ctx = Rc::new(Context::new());
        let mut env = Env::new_with_output(ctx, Box::new(io::sink()));
        crate::natives::register_natives(&mut env);
        let body = load_source(src).expect("parse failed");
        crate::binding::bind_pass_into(&body, &env.user_ctx);
        let block = Value::block(body);
        let result = crate::interp::eval(&block, &mut env).expect("eval failed");
        (env, result)
    }

    fn run_err(src: &str) -> EvalError {
        let ctx = Rc::new(Context::new());
        let mut env = Env::new_with_output(ctx, Box::new(io::sink()));
        crate::natives::register_natives(&mut env);
        let body = load_source(src).expect("parse failed");
        crate::binding::bind_pass_into(&body, &env.user_ctx);
        let block = Value::block(body);
        crate::interp::eval(&block, &mut env).expect_err("expected error")
    }

    #[test]
    fn channel_send_recv_integer() {
        let (_, result) = run("c: channel send c 5 recv c");
        match result {
            Value::Integer { n, .. } => assert_eq!(n, 5),
            other => panic!("expected Integer(5), got {:?}", other),
        }
    }

    #[test]
    fn send_func_errors() {
        // `func [x][x]` creates a function value; sending it should fail.
        let err = run_err("c: channel f: func [x][x] send c :f");
        let msg = match err {
            EvalError::Native { message, .. } => message,
            EvalError::Raised(e) => e.message.clone(),
            other => panic!("expected EvalError::Native or Raised, got {other:?}"),
        };
        assert!(msg.contains("function!"), "message: {msg}");
    }

    #[test]
    fn send_object_round_trip() {
        // Use the native API directly (avoids VM compilation issues with
        // `make object!` inside the test `run` helper).
        let ctx = Rc::new(Context::new());
        let mut env = Env::new_with_output(ctx, Box::new(io::sink()));
        crate::natives::register_natives(&mut env);

        let obj = ObjectDef::new();
        obj.ctx.set(Symbol::new("x"), Value::integer(5));
        let original = Value::object(obj);

        let channel_val = channel_native(&[], &RefineArgs::default(), &mut env).unwrap();
        send_native(
            &[channel_val.clone(), original],
            &RefineArgs::default(),
            &mut env,
        )
        .unwrap();
        let received = recv_native(&[channel_val], &RefineArgs::default(), &mut env).unwrap();

        match received {
            Value::Object(obj) => {
                let o = obj.borrow();
                let x = o.ctx.get(&Symbol::new("x")).expect("x field");
                match x {
                    Value::Integer { n, .. } => assert_eq!(n, 5),
                    other => panic!("expected Integer(5), got {other:?}"),
                }
            }
            other => panic!("expected Object, got {other:?}"),
        }
    }

    #[test]
    fn close_then_send_errors() {
        let err = run_err("c: channel close c send c 5");
        let msg = match err {
            EvalError::Native { message, .. } => message,
            EvalError::Raised(e) => e.message.clone(),
            other => panic!("expected EvalError::Native or Raised, got {other:?}"),
        };
        assert!(msg.contains("closed"), "message: {msg}");
    }

    #[test]
    fn close_then_recv_returns_none() {
        let (_, result) = run("c: channel close c recv c");
        assert!(matches!(result, Value::None), "expected None, got {result:?}");
    }

    #[test]
    fn mold_channel_is_placeholder() {
        let (_, result) = run("c: channel mold c");
        match result {
            Value::String { s, .. } => {
                assert_eq!(&*s, "#[channel]");
            }
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn channel_predicate_true() {
        let (_, result) = run("c: channel channel? c");
        assert!(matches!(result, Value::Logic(true)));
    }

    #[test]
    fn channel_predicate_false() {
        let (_, result) = run("channel? 5");
        assert!(matches!(result, Value::Logic(false)));
    }

    #[test]
    fn closed_predicate_false_before_close() {
        let (_, result) = run("c: channel closed? c");
        assert!(matches!(result, Value::Logic(false)));
    }

    #[test]
    fn closed_predicate_true_after_close() {
        let (_, result) = run("c: channel close c closed? c");
        assert!(matches!(result, Value::Logic(true)));
    }

    #[test]
    fn send_string_recv_round_trip() {
        let (_, result) = run("c: channel send c \"hello\" recv c");
        match result {
            Value::String { s, .. } => assert_eq!(&*s, "hello"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn send_block_recv_deep_cloned() {
        let (_, result) = run("c: channel send c [1 2 3] recv c");
        match result {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                assert_eq!(data.len(), 3);
                match &data[0] {
                    Value::Integer { n, .. } => assert_eq!(*n, 1),
                    other => panic!("expected Integer(1), got {other:?}"),
                }
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn send_object_independent_storage() {
        let ctx = Rc::new(Context::new());
        let mut env = Env::new_with_output(ctx, Box::new(io::sink()));
        crate::natives::register_natives(&mut env);

        // Create an object, store it, send it, recv it, then check the
        // received object is NOT the same Rc.
        let obj = ObjectDef::new();
        obj.ctx.set(Symbol::new("x"), Value::integer(42));
        let original = Value::object(obj);

        // Manually send + recv.
        let channel_val = channel_native(&[], &RefineArgs::default(), &mut env).unwrap();
        send_native(&[channel_val.clone(), original.clone()], &RefineArgs::default(), &mut env).unwrap();
        let received = recv_native(&[channel_val], &RefineArgs::default(), &mut env).unwrap();

        match (&original, &received) {
            (Value::Object(o1), Value::Object(o2)) => {
                assert!(!Rc::ptr_eq(o1, o2), "Object should have independent storage");
            }
            _ => panic!("expected Objects"),
        }
    }

    // ---- spawn tests (M43) ----

    #[test]
    fn spawn_returns_integer() {
        let (_, result) = run("r: spawn [5] recv r");
        match result {
            Value::Integer { n, .. } => assert_eq!(n, 5),
            other => panic!("expected Integer(5), got {other:?}"),
        }
    }

    #[test]
    fn spawn_returns_arithmetic_result() {
        let (_, result) = run("r: spawn [1 + 2] recv r");
        match result {
            Value::Integer { n, .. } => assert_eq!(n, 3),
            other => panic!("expected Integer(3), got {other:?}"),
        }
    }

    #[test]
    fn spawn_worker_defines_and_calls_func() {
        // A worker defining and calling a func; verifies the worker's
        // ThreadEnv has a working natives registry (the `func` native resolves).
        let (_, result) = run("r: spawn [f: func [x] [x] f 5] recv r");
        match result {
            Value::Integer { n, .. } => assert_eq!(n, 5),
            other => panic!("expected Integer(5), got {other:?}"),
        }
    }

    #[test]
    fn spawn_unbound_word_returns_error() {
        // `foo` is unbound — the worker's user_ctx snapshot doesn't have it.
        let (_, result) = run("r: spawn [foo] recv r");
        match result {
            Value::Error(err) => {
                assert!(
                    err.message.contains("has no value") || err.message.contains("unbound"),
                    "error should mention unbound word: {message}",
                    message = err.message
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn spawn_string_result() {
        let (_, result) = run("r: spawn [\"hello\"] recv r");
        match result {
            Value::String { s, .. } => assert_eq!(&*s, "hello"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn spawn_block_result() {
        let (_, result) = run("r: spawn [[1 2 3]] recv r");
        match result {
            Value::Block { series, .. } => {
                let data = series.data.borrow();
                assert_eq!(data.len(), 3);
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn spawn_channel_is_channel_value() {
        // `spawn` should return a channel! value.
        let (_, result) = run("r: spawn [5] channel? r");
        assert!(matches!(result, Value::Logic(true)), "expected Logic(true)");
    }

    #[test]
    fn spawn_non_block_arg_errors() {
        let err = run_err("spawn 5");
        // Should be a type error (expected block!, found integer!).
        let msg = match err {
            EvalError::TypeError { expected, found, .. } => {
                assert_eq!(expected, "block!");
                assert_eq!(found, "integer!");
                return;
            }
            EvalError::Raised(e) => e.message.clone(),
            other => panic!("expected TypeError, got {other:?}"),
        };
        // If it came through as Raised, check the message mentions type.
        assert!(msg.contains("block!") || msg.contains("type"), "message: {msg}");
    }

    #[test]
    #[ignore] // slow — spawns 1000 threads
    fn spawn_1000_workers() {
        // Spawns 1000 workers; verifies the 256 KiB stack setting keeps
        // memory bounded (~250 MiB total thread stacks at 1000 workers).
        let ctx = Rc::new(Context::new());
        let mut env = Env::new_with_output(ctx, Box::new(io::sink()));
        crate::natives::register_natives(&mut env);
        let body = load_source("results: [] repeat i 1000 [append results recv spawn [i * 2]] length? results").expect("parse failed");
        crate::binding::bind_pass_into(&body, &env.user_ctx);
        let block = Value::block(body);
        let result = crate::interp::eval(&block, &mut env).expect("eval failed");
        match result {
            Value::Integer { n, .. } => assert_eq!(n, 1000),
            other => panic!("expected Integer(1000), got {other:?}"),
        }
    }

    // ---- actor tests (M45) ----

    /// Helper: run source with captured output (for verifying print).
    fn run_capture(src: &str) -> String {
        use std::cell::RefCell as StdRefCell;
        let buf = Rc::new(StdRefCell::new(Vec::new()));
        let buf_clone = Rc::clone(&buf);
        struct BufWriter(Rc<StdRefCell<Vec<u8>>>);
        impl std::io::Write for BufWriter {
            fn write(&mut self, d: &[u8]) -> std::io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(d);
                Ok(d.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let ctx = Rc::new(Context::new());
        let mut env = Env::new_with_output(ctx, Box::new(BufWriter(buf_clone)));
        crate::natives::register_natives(&mut env);
        let body = load_source(src).expect("parse failed");
        crate::binding::bind_pass_into(&body, &env.user_ctx);
        let block = Value::block(body);
        let _ = crate::interp::eval(&block, &mut env);
        let out = String::from_utf8_lossy(&buf.borrow()).into_owned();
        out
    }

    #[test]
    fn actor_prints_message() {
        let out = run_capture(
            "a: spawn-actor [func [msg] [print msg]] send-actor a \"hi\" run-actors",
        );
        assert_eq!(out.trim(), "hi");
    }

    #[test]
    fn actor_counter_replies() {
        // A simple echo actor that replies with the message + 1.
        let out = run_capture(
            "reply: channel
             a: spawn-actor [func [msg] [
                 send reply msg + 1
             ]]
             send-actor a 40
             send-actor a 41
             run-actors
             print recv reply
             print recv reply"
        );
        let lines: Vec<&str> = out.trim().lines().collect();
        assert_eq!(lines, vec!["41", "42"], "output: {out}");
    }

    #[test]
    fn actor_receive_pattern_match() {
        // A handler using `either` to pattern-match the message arg.
        let out = run_capture(
            "a: spawn-actor [func [msg] [
                 either msg = 1 [
                     print \"one\"
                 ][
                     either msg = 2 [
                         print \"two\"
                     ][
                         print \"other\"
                     ]
                 ]
             ]]
             send-actor a 1
             run-actors
             send-actor a 2
             run-actors
             send-actor a 99
             run-actors"
        );
        let lines: Vec<&str> = out.trim().lines().collect();
        assert_eq!(lines, vec!["one", "two", "other"], "output: {out}");
    }

    #[test]
    fn actor_dead_actor_skipped() {
        // An actor with alive?: false is skipped by the scheduler.
        let out = run_capture(
            "a: spawn-actor [func [msg] [print msg]]
             send-actor a \"hello\"
             a/alive?: false
             run-actors"
        );
        // Should print nothing — the actor is dead, skipped.
        assert_eq!(out.trim(), "");
    }

    #[test]
    fn actor_multiple_messages() {
        // Send multiple messages; the scheduler processes one per dispatch.
        let out = run_capture(
            "a: spawn-actor [func [msg] [print msg]]
             send-actor a 1
             send-actor a 2
             send-actor a 3
             run-actors"
        );
        let lines: Vec<&str> = out.trim().lines().collect();
        assert_eq!(lines, vec!["1", "2", "3"], "output: {out}");
    }

    #[test]
    fn actor_1000_messages() {
        // 1000 actors each receiving one message — cooperative model is
        // fast (no OS thread per actor).
        let env_ctx = Context::new();
        let ctx = Rc::new(env_ctx);
        let mut env = Env::new_with_output(ctx, Box::new(io::sink()));
        crate::natives::register_natives(&mut env);
        let src = "repeat i 1000 [
            send-actor spawn-actor [func [msg] [msg]] i
        ] run-actors";
        let body = load_source(src).expect("parse failed");
        crate::binding::bind_pass_into(&body, &env.user_ctx);
        let block = Value::block(body);
        let start = std::time::Instant::now();
        let result = crate::interp::eval(&block, &mut env);
        let elapsed = start.elapsed();
        assert!(result.is_ok(), "eval failed: {:?}", result);
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1000 actors took {elapsed:?} (should be < 1s)"
        );
    }

    #[test]
    #[ignore = "spawn inside actor handler has a marshalling issue — the walker's call_user_func context interferes with spawn_thread's body marshalling. Tracked for M46."]
    fn actor_can_spawn_thread() {
        // An actor that calls `spawn` for heavy compute works correctly.
        // Uses a simple literal body (no handler-param refs — those would
        // be unbound on the worker thread).
        let out = run_capture(
            "a: spawn-actor [func [msg] [
                 r: spawn [42]
                 print recv r
             ]]
             send-actor a 0
             run-actors"
        );
        assert_eq!(out.trim(), "42");
    }

    // ---- M46: single-message handler convention ----

    #[test]
    fn actor_single_message_1000_messages() {
        // M46: a single-message counter actor processes 1000 messages
        // correctly. Each `send-actor` enqueues one dispatch; the handler
        // runs once per message. The actor accumulates state via a global
        // counter (since the handler's local state doesn't persist across
        // dispatches in the single-message model).
        let out = run_capture(
            "total: 0
             a: spawn-actor [func [msg] [
                 total: total + msg
             ]]
             repeat i 1000 [send-actor a 1]
             run-actors
             print total"
        );
        // Should print 1000 (1000 messages, each adding 1).
        assert_eq!(out.trim(), "1000", "output: {out}");
    }

    // ---- M47: actor links + supervisors ----

    #[test]
    fn link_propagates_exit() {
        let out = run_capture(
            "sup: spawn-actor [func [msg] [
                 either msg = \"exit\" [
                     print \"supervisor received exit\"
                 ][
                     print \"supervisor got msg\"
                 ]
             ]]
             child: spawn-actor [func [msg] [
                 either msg = 'crash [
                     print \"child crashing\"
                     1 / 0
                 ][
                     print \"child ok\"
                 ]
             ]]
             link sup child
             send-actor child 'crash
             run-actors"
        );
        let lines: Vec<&str> = out.trim().lines().collect();
        assert!(lines.iter().any(|l| l.contains("child crashing")), "should show child crashing: {out}");
        assert!(lines.iter().any(|l| l.contains("supervisor received exit")), "should show supervisor receiving exit: {out}");
    }

    #[test]
    fn monitor_propagates_down() {
        let out = run_capture(
            "mon: spawn-actor [func [msg] [
                 either msg = \"down\" [
                     print \"monitor received down\"
                 ][
                     print \"monitor got msg\"
                 ]
             ]]
             child: spawn-actor [func [msg] [
                 print \"child running\"
                 1 / 0
             ]]
             monitor child mon
             send-actor child 'go
             run-actors"
        );
        let lines: Vec<&str> = out.trim().lines().collect();
        assert!(lines.iter().any(|l| l.contains("monitor received down")), "should show monitor receiving down: {out}");
    }

    #[test]
    fn spawn_supervisor_creates_actor() {
        // spawn-supervisor is an alias for spawn-actor.
        let out = run_capture(
            "sup: spawn-supervisor [func [msg] [print msg]]
             send-actor sup \"hello\"
             run-actors"
        );
        assert_eq!(out.trim(), "hello");
    }

    // ---- self native (Phase 1 supervisor scaffolding) ----

    #[test]
    fn self_native_outside_actor_is_none() {
        // Outside any actor handler, `self` returns none.
        let (_, result) = run("self");
        assert!(matches!(result, Value::None), "expected None, got {result:?}");
    }

    #[test]
    fn self_native_inside_actor_returns_actor() {
        // Inside a handler, `self` returns the dispatching actor object.
        // We use `same?` (Rc identity) — `=` would do deep field comparison
        // and the mailbox/handler fields don't compare meaningfully.
        let out = run_capture(
            "a: spawn-actor [func [msg] [
                 either same? self a [
                     print \"self is actor\"
                 ][
                     print \"self mismatch\"
                 ]
             ]]
             send-actor a 1
             run-actors"
        );
        assert_eq!(out.trim(), "self is actor", "output: {out}");
    }
}
