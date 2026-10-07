/// The database namespace this module owns: a PostgreSQL schema, a MySQL
/// database, or the SQLite file ATTACHed under that name. Never write outside
/// it.
pub const SCHEMA: &str = "mail";

pub mod config;
pub mod db;
pub mod errors;
pub mod events;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod router;
/// SMTP/IMAP/POP3 services this instance offers (as opposed to connects to).
pub mod server;
pub mod services;
pub mod state;
pub mod workers;
