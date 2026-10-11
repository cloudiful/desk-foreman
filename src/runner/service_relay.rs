//! Owner-scoped reverse-relay registry (issue 749 P2b).
//!
//! Thin module entry: the tunnel handle, wire conversion, per-stream call
//! ticket, admission refusals and the registry itself live in the sibling
//! directory so this file stays wiring-only.
//!
//! This module is network bookkeeping and nothing else. It is not the
//! lifecycle authority: the database remains the single source of truth for
//! which environment generation may serve, and every stream opened here is
//! re-checked against it before the tunnel is told anything.

pub mod admit_check;
pub mod call;
pub mod closed;
pub mod errors;
pub mod frames;
#[cfg(test)]
mod generation_cases;
pub mod registry;
#[cfg(test)]
mod registry_burst_tests;
pub mod registry_lifecycle;
#[cfg(test)]
mod registry_multiplex_tests;
mod registry_state;
#[cfg(test)]
mod registry_tests;
pub mod tunnel;

pub use call::RelayCall;
pub use errors::{RelayAdmissionError, RelayCallError};
pub use frames::{InboundEvent, OutboundFrame, StreamDelivery};
pub use registry::{RelayRegistry, StreamRequest};
pub use tunnel::{TUNNEL_OUTBOUND_CAPACITY, TunnelHandle};
