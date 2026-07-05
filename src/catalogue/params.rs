//! Converts `sea_query::Value` (bound values produced by any
//! `sea-query`-built statement in this module) into `turso::Value`
//! (accepted by `turso::Connection::query`/`execute`).
//!
//! A free function, not a `From`/`TryFrom` impl: neither type is local to
//! this crate, so a trait impl would violate Rust's orphan rules.

/// Converts every bound value from a compiled statement, in order, into
/// `turso::Value`s ready to pass as `Vec<turso::Value>` query/execute
/// params.
///
/// # Panics
///
/// Panics on any `sea_query::Value` variant this crate's query-building
/// code never actually constructs. Today that's everything except:
/// - `String` - text columns and `LikeExpr` patterns.
/// - `Int` - small integer literals (e.g. `is_primary = 1`).
/// - `BigInt` - `i64` ids (`ScoreId`/`TitleId`, `last_insert_rowid()`).
/// - `BigUnsigned` - `SelectStatement::limit`/`.offset` (both take `u64`).
///
/// If this crate starts binding a new kind of value, extend this function
/// to cover it.
pub(super) fn to_turso_values(values: sea_query::Values) -> Vec<turso::Value> {
    values.0.into_iter().map(to_turso_value).collect()
}

fn to_turso_value(value: sea_query::Value) -> turso::Value {
    match value {
        sea_query::Value::String(s) => s.map_or(turso::Value::Null, turso::Value::Text),
        sea_query::Value::Int(i) => {
            i.map_or(turso::Value::Null, |i| turso::Value::Integer(i64::from(i)))
        }
        sea_query::Value::BigInt(i) => i.map_or(turso::Value::Null, turso::Value::Integer),
        sea_query::Value::BigUnsigned(i) => i.map_or(turso::Value::Null, |i| {
            turso::Value::Integer(
                i64::try_from(i).expect("only ever binds BigUnsigned for LIMIT/OFFSET"),
            )
        }),
        other => unreachable!(
            "query-building never binds a {other:?} value; extend this function if that changes"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_string_int_bigint_bigunsigned() {
        let values = sea_query::Values(vec![
            sea_query::Value::String(Some("ave%".to_string())),
            sea_query::Value::Int(Some(1)),
            sea_query::Value::BigInt(Some(42)),
            sea_query::Value::BigUnsigned(Some(10)),
            sea_query::Value::String(None),
        ]);

        assert_eq!(
            to_turso_values(values),
            vec![
                turso::Value::Text("ave%".to_string()),
                turso::Value::Integer(1),
                turso::Value::Integer(42),
                turso::Value::Integer(10),
                turso::Value::Null,
            ]
        );
    }
}
