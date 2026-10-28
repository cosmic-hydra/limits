//! Small eager data operations for concise agent-written programs. These
//! builtins have no ambient I/O and never execute callbacks or mutate arguments.
use crate::ast::BinOp;
use crate::error::{Kind, LimError, Span};
use crate::interp::{self, Value};

pub const MAX_COLLECTION_WORK: usize = 1_000_000;

fn error(message: impl Into<String>, span: Span) -> LimError {
    LimError::new(Kind::Runtime, message, span)
}
fn arity(name: &str, args: &[Value], n: usize, span: Span) -> Result<(), LimError> {
    if args.len() == n {
        Ok(())
    } else {
        Err(error(
            format!("{name}() takes {n} argument(s), got {}", args.len()),
            span,
        ))
    }
}
/// Invoke one of the standard data helpers. Source-level calls use the same
/// implementation after invocation policy and normal call budget checks.
pub fn call(name: &str, args: Vec<Value>, span: Span) -> Result<Value, LimError> {
    match name {
        "get" | "has" => {
            if name == "get" {
                if !(2..=3).contains(&args.len()) {
                    return Err(error("get() takes 2 or 3 arguments", span));
                }
            } else {
                arity(name, &args, 2, span)?;
            }
            let Value::Map(map) = &args[0] else {
                return Err(error(
                    format!("{name}() expects a map, got {}", args[0].type_name()),
                    span,
                ));
            };
            let Value::Str(key) = &args[1] else {
                return Err(error(
                    format!("{name}() key must be a string, got {}", args[1].type_name()),
                    span,
                ));
            };
            let map = map.borrow();
            if name == "has" {
                Ok(Value::Bool(map.contains_key(&**key)))
            } else {
                Ok(map
                    .get(&**key)
                    .cloned()
                    .unwrap_or_else(|| args.get(2).cloned().unwrap_or(Value::Nil)))
            }
        }
        "join" => {
            arity(name, &args, 2, span)?;
            let Value::List(list) = &args[0] else {
                return Err(error(
                    format!("join() expects a list, got {}", args[0].type_name()),
                    span,
                ));
            };
            let Value::Str(separator) = &args[1] else {
                return Err(error(
                    format!(
                        "join() separator must be a string, got {}",
                        args[1].type_name()
                    ),
                    span,
                ));
            };
            let list = list.borrow();
            if list.len() > MAX_COLLECTION_WORK {
                return Err(error(
                    "join() element limit exceeded (1000000 elements)",
                    span,
                ));
            }
            let output_limit = crate::value::MAX_RENDER_BYTES;
            let mut bytes = separator
                .len()
                .checked_mul(list.len().saturating_sub(1))
                .ok_or_else(|| error("join() output limit exceeded (16777216 bytes)", span))?;
            for value in list.iter() {
                let Value::Str(text) = value else {
                    return Err(error(
                        format!("join() expects string elements, got {}", value.type_name()),
                        span,
                    ));
                };
                bytes = bytes
                    .checked_add(text.len())
                    .filter(|&n| n <= output_limit)
                    .ok_or_else(|| error("join() output limit exceeded (16777216 bytes)", span))?;
            }
            if bytes > output_limit {
                return Err(error("join() output limit exceeded (16777216 bytes)", span));
            }
            let mut text = String::new();
            text.try_reserve_exact(bytes)
                .map_err(|_| error("join() allocation exceeds host capacity", span))?;
            for (i, value) in list.iter().enumerate() {
                if i > 0 {
                    text.push_str(separator);
                }
                let Value::Str(value) = value else {
                    unreachable!("validated string elements")
                };
                text.push_str(value);
            }
            Ok(Value::Str(text.into()))
        }
        "sum" => {
            arity(name, &args, 1, span)?;
            let Value::List(list) = &args[0] else {
                return Err(error(
                    format!("sum() expects a list, got {}", args[0].type_name()),
                    span,
                ));
            };
            let list = list.borrow();
            if list.len() > MAX_COLLECTION_WORK {
                return Err(error(
                    "sum() element limit exceeded (1000000 elements)",
                    span,
                ));
            }
            let mut total = Value::Int(0);
            for value in list.iter() {
                if !matches!(value, Value::Int(_) | Value::Float(_)) {
                    return Err(error(
                        format!("sum() expects numeric elements, got {}", value.type_name()),
                        span,
                    ));
                }
                total = interp::binary(BinOp::Add, total, value.clone(), span)?;
            }
            Ok(total)
        }
        _ => Err(error(format!("unknown standard builtin '{name}'"), span)),
    }
}
