//! Integration coverage for the owner-scoped private-service gateway
//! (issue 749 P2b).
//!
//! Drives the real internal HTTP surface against an isolated nonproduction
//! PostgreSQL created from `DF_ENV_TEST_DATABASE_URL`. The manager side is a
//! mock that holds a real relay tunnel and answers with real relay frames:
//! these cases prove the control plane's own behaviour and deliberately prove
//! nothing about Docker, a real agent image, or a real outbound manager
//! client.

#[path = "execution_environments/helpers.rs"]
mod helpers;

#[path = "execution_environments/http.rs"]
mod http;

#[path = "execution_environment_service/support.rs"]
mod support;

#[path = "execution_environment_service/ws_client.rs"]
mod ws_client;

#[path = "execution_environment_service/admission_cases.rs"]
mod admission_cases;

#[path = "execution_environment_service/relay_helpers.rs"]
mod relay_helpers;

#[path = "execution_environment_service/relay_cases.rs"]
mod relay_cases;

#[path = "execution_environment_service/relay_cancel_cases.rs"]
mod relay_cancel_cases;

#[path = "execution_environment_service/transport_bound_cases.rs"]
mod transport_bound_cases;
