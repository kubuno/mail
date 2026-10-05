//! Statement-level bridge from the sqlx call shape onto `kubuno-db`.
//!
//! The mail module was written against `sqlx::query(...).bind(..).fetch_one(&pool)`
//! with a `PgPool`. `kubuno-db` executes through a run-time engine enum
//! ([`DbPool`] / [`DbTx`]) that takes a finished SQL string plus a `Vec<DbValue>`.
//! This module keeps the familiar builder shape — `db::query`, `db::query_as`,
//! `db::query_scalar`, `.bind()`, `.execute()`, `.fetch_one()` … — and hands the
//! statement to the engine-agnostic executor, so the ~470 statements of the
//! module read the same as before while running on PostgreSQL, MySQL/MariaDB
//! and SQLite.
//!
//! What it adds on top of the executor:
//!
//! * **Placeholder renumbering.** `kubuno-db` refuses a placeholder that is
//!   reused or out of order (`?` is positional on MySQL/SQLite). Statements here
//!   may reuse `$1`: the bridge rewrites the text to strictly increasing `$n` and
//!   duplicates the bound values to match, outside string literals, quoted
//!   identifiers and comments. The SQL is otherwise passed through unchanged —
//!   this is not a dialect translator; dialect differences are spelled with
//!   `kubuno_db::dialect` at the call site.
//! * **One terminal for pool and transaction.** A terminal accepts `&DbPool` or
//!   `&mut DbTx` ([`Ex`]). Multi-row reads (`fetch_all`) exist on the pool only,
//!   which is all the executor offers inside a transaction.
//!
//! User data only ever travels as a bind ([`DbValue`]); the SQL text is the
//! developer's.

use std::marker::PhantomData;

use chrono::{DateTime, NaiveDate, Utc};
use kubuno_db::{DbPool, DbRow, DbTx, DbValue, FromAnyRow, ScalarAnyRow};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// Where a statement runs: the pool, or an open transaction.
pub enum Ex<'a> {
    Pool(&'a DbPool),
    Tx(&'a mut DbTx),
}

impl<'a> From<&'a DbPool> for Ex<'a> {
    fn from(p: &'a DbPool) -> Self {
        Ex::Pool(p)
    }
}

impl<'a> From<&'a mut DbTx> for Ex<'a> {
    fn from(t: &'a mut DbTx) -> Self {
        Ex::Tx(t)
    }
}

impl<'a, 'b> From<&'a mut &'b mut DbTx> for Ex<'a> {
    fn from(t: &'a mut &'b mut DbTx) -> Self {
        Ex::Tx(t)
    }
}

/// Result of a write, mirroring `sqlx`'s `rows_affected()` accessor.
#[derive(Debug, Clone, Copy)]
pub struct Done(u64);

impl Done {
    pub fn rows_affected(&self) -> u64 {
        self.0
    }
}

/// Marker: rows are returned as [`DbRow`].
pub struct Untyped;
/// Marker: rows decode into `T` (`#[derive(sqlx::FromRow)]` or a tuple).
pub struct As<T>(PhantomData<T>);
/// Marker: the first column decodes into `T`.
pub struct Scalar<T>(PhantomData<T>);

/// A statement and its binds, not yet executed.
pub struct Query<O> {
    sql: String,
    params: Vec<DbValue>,
    _out: PhantomData<O>,
}

/// An untyped statement (writes, or reads mapped by hand from [`DbRow`]).
pub fn query(sql: impl SqlText) -> Query<Untyped> {
    Query { sql: sql.into_sql(), params: Vec::new(), _out: PhantomData }
}

/// A read whose rows decode into `T`.
pub fn query_as<T>(sql: impl SqlText) -> Query<As<T>> {
    Query { sql: sql.into_sql(), params: Vec::new(), _out: PhantomData }
}

/// A read whose first column decodes into `T`.
pub fn query_scalar<T>(sql: impl SqlText) -> Query<Scalar<T>> {
    Query { sql: sql.into_sql(), params: Vec::new(), _out: PhantomData }
}

impl<O> Query<O> {
    /// Binds the next `$n` value.
    pub fn bind(mut self, v: impl Bind) -> Self {
        self.params.push(v.into_value());
        self
    }

    /// The SQL text and binds, placeholders renumbered strictly increasing.
    fn parts(self) -> Result<(String, Vec<DbValue>), sqlx::Error> {
        renumber(&self.sql, self.params)
    }
}

impl Query<Untyped> {
    pub async fn execute<'a>(self, ex: impl Into<Ex<'a>>) -> Result<Done, sqlx::Error> {
        let (sql, params) = self.parts()?;
        let n = match ex.into() {
            Ex::Pool(p) => p.execute(&sql, params).await?,
            Ex::Tx(t) => t.execute(&sql, params).await?,
        };
        Ok(Done(n))
    }

    pub async fn fetch_optional<'a>(
        self,
        ex: impl Into<Ex<'a>>,
    ) -> Result<Option<DbRow>, sqlx::Error> {
        let (sql, params) = self.parts()?;
        match ex.into() {
            Ex::Pool(p) => p.fetch_optional_row(&sql, params).await,
            Ex::Tx(t) => t.fetch_optional_row(&sql, params).await,
        }
    }

    pub async fn fetch_one<'a>(self, ex: impl Into<Ex<'a>>) -> Result<DbRow, sqlx::Error> {
        self.fetch_optional(ex).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn fetch_all(self, pool: &DbPool) -> Result<Vec<DbRow>, sqlx::Error> {
        let (sql, params) = self.parts()?;
        pool.fetch_all_row(&sql, params).await
    }
}

impl<T: FromAnyRow> Query<As<T>> {
    pub async fn fetch_optional<'a>(self, ex: impl Into<Ex<'a>>) -> Result<Option<T>, sqlx::Error> {
        let (sql, params) = self.parts()?;
        match ex.into() {
            Ex::Pool(p) => p.fetch_optional_as(&sql, params).await,
            Ex::Tx(t) => match t.fetch_optional_row(&sql, params).await? {
                Some(row) => Ok(Some(decode_row::<T>(&row)?)),
                None => Ok(None),
            },
        }
    }

    pub async fn fetch_one<'a>(self, ex: impl Into<Ex<'a>>) -> Result<T, sqlx::Error> {
        self.fetch_optional(ex).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn fetch_all(self, pool: &DbPool) -> Result<Vec<T>, sqlx::Error> {
        let (sql, params) = self.parts()?;
        pool.fetch_all_as(&sql, params).await
    }
}

impl<T: ScalarAnyRow> Query<Scalar<T>> {
    pub async fn fetch_optional<'a>(self, ex: impl Into<Ex<'a>>) -> Result<Option<T>, sqlx::Error> {
        let (sql, params) = self.parts()?;
        match ex.into() {
            Ex::Pool(p) => p.fetch_optional_scalar(&sql, params).await,
            Ex::Tx(t) => t.fetch_optional_scalar(&sql, params).await,
        }
    }

    pub async fn fetch_one<'a>(self, ex: impl Into<Ex<'a>>) -> Result<T, sqlx::Error> {
        self.fetch_optional(ex).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn fetch_all(self, pool: &DbPool) -> Result<Vec<T>, sqlx::Error> {
        let (sql, params) = self.parts()?;
        let rows: Vec<(T,)> = pool.fetch_all_as(&sql, params).await?;
        Ok(rows.into_iter().map(|(v,)| v).collect())
    }
}

/// Decodes a hand-fetched row into `T` on whichever engine produced it.
pub fn decode_row<T: FromAnyRow>(row: &DbRow) -> Result<T, sqlx::Error> {
    match row {
        DbRow::Pg(r) => <T as sqlx::FromRow<'_, sqlx::postgres::PgRow>>::from_row(r),
        DbRow::My(r) => <T as sqlx::FromRow<'_, sqlx::mysql::MySqlRow>>::from_row(r),
        DbRow::Sq(r) => <T as sqlx::FromRow<'_, sqlx::sqlite::SqliteRow>>::from_row(r),
    }
}

/// Rewrites `$n` placeholders so they appear strictly increasing (`$1`, `$2`, …)
/// in text order, duplicating or reordering the bound values to match. Text
/// inside single-quoted literals, double-quoted identifiers and comments is left
/// alone. A placeholder with no bound value is an error, never a silent NULL.
pub fn renumber(sql: &str, params: Vec<DbValue>) -> Result<(String, Vec<DbValue>), sqlx::Error> {
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut order: Vec<usize> = Vec::new();
    let mut i = 0;
    let mut start = 0;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' => {
                let q = b[i];
                i += 1;
                while i < b.len() {
                    if b[i] == q {
                        // A doubled quote is an escaped quote, not the end.
                        if i + 1 < b.len() && b[i + 1] == q {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'-' if i + 1 < b.len() && b[i + 1] == b'-' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'$' if i + 1 < b.len() && b[i + 1].is_ascii_digit() => {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                let n: usize = sql[i + 1..j]
                    .parse()
                    .map_err(|_| sqlx::Error::Protocol(format!("bad placeholder in: {sql}")))?;
                if n == 0 || n > params.len() {
                    return Err(sqlx::Error::Protocol(format!(
                        "placeholder ${n} has no bound value ({} bound)",
                        params.len()
                    )));
                }
                out.push_str(&sql[start..i]);
                order.push(n - 1);
                out.push('$');
                out.push_str(&order.len().to_string());
                i = j;
                start = j;
            }
            _ => i += 1,
        }
    }
    out.push_str(&sql[start.min(sql.len())..]);
    // Fast path: already 1..=n in order with every bind used exactly once.
    if order.len() == params.len() && order.iter().enumerate().all(|(k, &n)| k == n) {
        return Ok((out, params));
    }
    let new_params = order.iter().map(|&n| params[n].clone()).collect();
    Ok((out, new_params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_placeholders_are_duplicated() {
        let (sql, p) = renumber(
            "SELECT 1 WHERE a = $1 AND b = $2 OR c = $1",
            vec![DbValue::from(7i64), DbValue::from("x")],
        )
        .unwrap();
        assert_eq!(sql, "SELECT 1 WHERE a = $1 AND b = $2 OR c = $3");
        assert_eq!(p, vec![DbValue::from(7i64), DbValue::from("x"), DbValue::from(7i64)]);
    }

    #[test]
    fn out_of_order_placeholders_are_reordered() {
        let (sql, p) = renumber("UPDATE t SET a = $2 WHERE id = $1", vec![1i64.into(), 2i64.into()]).unwrap();
        assert_eq!(sql, "UPDATE t SET a = $1 WHERE id = $2");
        assert_eq!(p, vec![DbValue::from(2i64), DbValue::from(1i64)]);
    }

    #[test]
    fn literals_and_comments_are_untouched() {
        let (sql, p) = renumber("SELECT '$1', \"$2\" -- $3\n FROM t WHERE x = $1", vec![5i64.into()]).unwrap();
        assert_eq!(sql, "SELECT '$1', \"$2\" -- $3\n FROM t WHERE x = $1");
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn a_missing_bind_is_an_error() {
        assert!(renumber("SELECT $2", vec![1i64.into()]).is_err());
    }
}

/// SQL text accepted by [`query`] & co.: developer-written literals, a
/// `format!`-built `String`, or the `sqlx::AssertSqlSafe` wrapper sqlx 0.9 asks
/// for around non-literal text.
pub trait SqlText {
    fn into_sql(self) -> String;
}

impl SqlText for &str {
    fn into_sql(self) -> String {
        self.to_owned()
    }
}
impl SqlText for String {
    fn into_sql(self) -> String {
        self
    }
}
impl SqlText for &String {
    fn into_sql(self) -> String {
        self.clone()
    }
}
impl SqlText for sqlx::AssertSqlSafe<String> {
    fn into_sql(self) -> String {
        self.0
    }
}
impl SqlText for sqlx::AssertSqlSafe<&str> {
    fn into_sql(self) -> String {
        self.0.to_owned()
    }
}
impl SqlText for sqlx::AssertSqlSafe<&String> {
    fn into_sql(self) -> String {
        self.0.clone()
    }
}

/// A value [`Query::bind`] accepts. `DbValue`'s own `From` impls cover owned
/// values; the borrowed shapes the call sites use (`&Option<String>`, `&bool`,
/// `&Vec<u8>`, `&&str`, `sqlx::types::Json<T>`) are added here, since the
/// orphan rule forbids adding `From` impls to a foreign type.
pub trait Bind {
    fn into_value(self) -> DbValue;
}

impl Bind for DbValue {
    fn into_value(self) -> DbValue {
        self
    }
}

macro_rules! bind_via_from {
    ($($t:ty),* $(,)?) => {$(
        impl Bind for $t {
            fn into_value(self) -> DbValue {
                DbValue::from(self)
            }
        }
    )*};
}

bind_via_from!(
    bool, i16, i32, i64, f32, f64, String, Vec<u8>, Uuid, JsonValue, DateTime<Utc>, NaiveDate,
    Option<bool>, Option<i16>, Option<i32>, Option<i64>, Option<f32>, Option<f64>,
    Option<String>, Option<Vec<u8>>, Option<Uuid>, Option<JsonValue>, Option<DateTime<Utc>>,
    Option<NaiveDate>, Option<&str>, &str, &String, &Uuid, &[u8],
    Vec<String>, &[String], &Vec<String>, Option<Vec<String>>,
    Vec<Uuid>, &[Uuid], &Vec<Uuid>, Option<Vec<Uuid>>,
);

/// Borrowed `Copy`/`Clone` values: bind the owned value.
macro_rules! bind_by_clone {
    ($($t:ty),* $(,)?) => {$(
        impl Bind for &$t {
            fn into_value(self) -> DbValue {
                DbValue::from(self.clone())
            }
        }
        impl Bind for &Option<$t> {
            fn into_value(self) -> DbValue {
                DbValue::from(self.clone())
            }
        }
    )*};
}

bind_by_clone!(bool, i16, i32, i64, f32, f64, Vec<u8>, JsonValue, DateTime<Utc>, NaiveDate);

impl Bind for &Option<String> {
    fn into_value(self) -> DbValue {
        DbValue::from(self.clone())
    }
}
impl Bind for &Option<Uuid> {
    fn into_value(self) -> DbValue {
        DbValue::from(*self)
    }
}
impl Bind for &&str {
    fn into_value(self) -> DbValue {
        DbValue::from(*self)
    }
}
impl Bind for Option<&String> {
    fn into_value(self) -> DbValue {
        DbValue::from(self.cloned())
    }
}
impl Bind for Option<&Uuid> {
    fn into_value(self) -> DbValue {
        DbValue::from(self.copied())
    }
}
impl Bind for &Option<Vec<String>> {
    fn into_value(self) -> DbValue {
        DbValue::from(self.clone())
    }
}

/// A `sqlx::types::Json<T>` wrapper binds as the JSON document of `T`.
impl<T: serde::Serialize> Bind for sqlx::types::Json<T> {
    fn into_value(self) -> DbValue {
        DbValue::Json(serde_json::to_value(&self.0).ok())
    }
}
impl<T: serde::Serialize> Bind for &sqlx::types::Json<T> {
    fn into_value(self) -> DbValue {
        DbValue::Json(serde_json::to_value(&self.0).ok())
    }
}
impl Bind for Option<&[u8]> {
    fn into_value(self) -> DbValue {
        DbValue::Blob(self.map(<[u8]>::to_vec))
    }
}

// ── Dialect helpers specific to this module ─────────────────────────────────
//
// `kubuno_db::dialect` covers the general cases; these are the few spellings the
// mail queries need that it does not offer. Every argument that is SQL text is
// developer-written (a column or expression), never request data.

use kubuno_db::Backend;

/// Case- and accent-insensitive substring/pattern match of `expr` against the
/// bound pattern `$n` (the caller adds the `%` wildcards).
///
/// * PostgreSQL: `unaccent(expr) ILIKE unaccent($n)` (the core installs the
///   `unaccent` extension) — unchanged from the PostgreSQL-only code.
/// * MySQL/MariaDB: `expr COLLATE utf8mb4_unicode_ci LIKE $n` — the pool's
///   default collation is binary, so the case/accent-insensitive Unicode
///   collation is named explicitly; it exists on both servers.
/// * SQLite: `expr LIKE $n` — SQLite's `LIKE` folds ASCII case only; accents
///   are not folded (no `unaccent` there). A documented limitation.
pub fn ci_like(backend: Backend, expr: &str, n: usize) -> String {
    match backend {
        Backend::Postgres => format!("unaccent({expr}) ILIKE unaccent(${n})"),
        Backend::MySql => format!("{expr} COLLATE utf8mb4_unicode_ci LIKE ${n}"),
        Backend::Sqlite => format!("{expr} LIKE ${n}"),
    }
}

/// Case-insensitive (not accent-insensitive) pattern match — PostgreSQL's
/// `ILIKE` without `unaccent`.
pub fn ilike(backend: Backend, expr: &str, n: usize) -> String {
    match backend {
        Backend::Postgres => format!("{expr} ILIKE ${n}"),
        Backend::MySql => format!("LOWER({expr}) LIKE LOWER(${n})"),
        Backend::Sqlite => format!("{expr} LIKE ${n}"),
    }
}

/// String concatenation: `||` is a logical OR on MySQL, and SQLite only has
/// `CONCAT()` since 3.44.
pub fn concat(backend: Backend, parts: &[&str]) -> String {
    match backend {
        Backend::MySql => format!("CONCAT({})", parts.join(", ")),
        Backend::Postgres | Backend::Sqlite => format!("({})", parts.join(" || ")),
    }
}

/// `GREATEST(a, b, …)`: SQLite spells it as the multi-argument scalar `MAX()`.
pub fn greatest(backend: Backend, parts: &[&str]) -> String {
    match backend {
        Backend::Sqlite => format!("MAX({})", parts.join(", ")),
        _ => format!("GREATEST({})", parts.join(", ")),
    }
}

/// `LEAST(a, b, …)`: SQLite spells it as the multi-argument scalar `MIN()`.
pub fn least(backend: Backend, parts: &[&str]) -> String {
    match backend {
        Backend::Sqlite => format!("MIN({})", parts.join(", ")),
        _ => format!("LEAST({})", parts.join(", ")),
    }
}

/// `FOR UPDATE SKIP LOCKED` for a queue claim. PostgreSQL and MySQL 8 /
/// MariaDB 10.6+ have it; SQLite has neither row locks nor the clause, and needs
/// none — its writers are serialised by the pool's single-writer gate, and the
/// claim runs inside a write transaction.
pub fn for_update_skip_locked(backend: Backend) -> &'static str {
    match backend {
        Backend::Sqlite => "",
        _ => " FOR UPDATE SKIP LOCKED",
    }
}

/// `FOR UPDATE` row lock (empty on SQLite, see [`for_update_skip_locked`]).
pub fn for_update(backend: Backend) -> &'static str {
    match backend {
        Backend::Sqlite => "",
        _ => " FOR UPDATE",
    }
}

/// True when the error is a unique-key violation, on any engine
/// (PostgreSQL 23505, MySQL 1062, SQLite 2067/1555).
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false)
}

/// True when the error is a foreign-key violation, on any engine.
pub fn is_foreign_key_violation(e: &sqlx::Error) -> bool {
    e.as_database_error().map(|d| d.is_foreign_key_violation()).unwrap_or(false)
}

/// True when the error is a CHECK-constraint violation, on any engine.
pub fn is_check_violation(e: &sqlx::Error) -> bool {
    e.as_database_error().map(|d| d.is_check_violation()).unwrap_or(false)
}

/// `$start, $start+1, …` for an `IN (…)` list of `count` binds. An empty list is
/// the caller's to short-circuit: `IN ()` is a syntax error on every engine.
pub fn in_list(start: usize, count: usize) -> String {
    (start..start + count).map(|i| format!("${i}")).collect::<Vec<_>>().join(", ")
}
