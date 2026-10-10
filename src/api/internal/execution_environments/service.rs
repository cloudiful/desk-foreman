//! Owner-scoped private-service gateway (issue 749 P2b).
//!
//! Thin module entry: request bounds, admission, and the streaming response
//! live in the sibling directory so this file stays wiring-only.
//!
//! The gateway is the tenant-facing half of the reverse relay. It decides
//! whether an application may reach one of its own environments, and then
//! streams one bounded HTTP request to that environment's private agent
//! through the environment's originally bound manager. It never learns a
//! manager address, an agent address, or a credential.

pub mod admission;
pub mod bounds;
pub mod handler;
pub mod relay;
pub mod response;

pub use bounds::BoundsRefusal;
pub use handler::service_route;

/// Header carrying the caller's expected environment generation.
///
/// This is a fence, not an identity: the bearer decides who is calling and
/// the database decides what that caller may reach.
pub const SERVICE_GENERATION_HEADER: &str = "x-execution-environment-generation";
