//! Integration coverage for durable execution environments (issue 737 P1).
//!
//! Database fencing cases live in `db_cases`, HTTP auth and contract
//! cases in `http_cases`; shared isolated-database helpers live in
//! `helpers`. Every case runs against an isolated nonproduction
//! PostgreSQL created from `DF_ENV_TEST_DATABASE_URL`.

#[path = "execution_environments/helpers.rs"]
mod helpers;

#[path = "execution_environments/http.rs"]
mod http;

#[path = "execution_environments/db_cases.rs"]
mod db_cases;

#[path = "execution_environments/http_cases.rs"]
mod http_cases;
