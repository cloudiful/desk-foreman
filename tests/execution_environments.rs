//! Integration coverage for durable execution environments (issue 737 P1).
//!
//! Database fencing cases live in `db_cases`, HTTP auth and contract
//! cases in `http_cases`, lease-free resource binding resolution in
//! `resource_binding_cases` with its fixtures in `resource_binding`;
//! the shared isolated database, application state and HTTP helpers live
//! in `isolated_db`, `app_state`, `app_token` and the `http_*` modules.
//! Every case runs against an isolated nonproduction PostgreSQL created
//! from `DF_ENV_TEST_DATABASE_URL`.

#[path = "execution_environments/test_database_url.rs"]
mod test_database_url;

#[path = "execution_environments/isolated_db.rs"]
mod isolated_db;

#[path = "execution_environments/app_state.rs"]
mod app_state;

#[path = "execution_environments/app_token.rs"]
mod app_token;

#[path = "execution_environments/http_server.rs"]
mod http_server;

#[path = "execution_environments/http_client.rs"]
mod http_client;

#[path = "execution_environments/http_get.rs"]
mod http_get;

#[path = "execution_environments/http_post.rs"]
mod http_post;

#[path = "execution_environments/db_cases.rs"]
mod db_cases;

#[path = "execution_environments/http_cases.rs"]
mod http_cases;

#[path = "execution_environments/resource_binding/fixtures.rs"]
mod resource_binding_fixtures;

#[path = "execution_environments/resource_binding_cases.rs"]
mod resource_binding_cases;
