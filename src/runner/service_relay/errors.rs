//! Admission and call refusals (issue 749 P2b).
//!
//! Refusals are closed enums rather than free-form strings: the gateway maps
//! each variant to one documented HTTP status, and the tunnel maps them to
//! fixed relay codes, so neither side has to parse diagnostic text.

use runner_protocol::service_relay::RelayProtocolError;

/// Why a stream could not be admitted to a manager tunnel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayAdmissionError {
    /// The originally bound manager has no live tunnel. A relay outage is
    /// temporary, so the gateway reports it as unavailable rather than as a
    /// client conflict.
    NoTunnel,
    /// This manager already has a live tunnel. Exactly one is admitted at a
    /// time; a second connection never silently replaces the first.
    TunnelConflict,
    /// The per-environment or per-manager concurrency cap is reached.
    StreamLimit,
    /// This stream identifier is already live or was recently closed on this
    /// tunnel. Reuse would make two streams unattributable, so the new
    /// request is refused rather than multiplexed onto a recycled id.
    DuplicateStream,
    /// No committed lifecycle mutation may register a stream for this
    /// environment any more; a registration that races a committed stop or
    /// invalidation is refused.
    EpochChanged,
    /// The declared body length is outside the frozen protocol bound.
    BodyTooLarge,
}

impl RelayAdmissionError {
    /// Fixed relay code, so a refusal is reportable without prose.
    pub fn code(self) -> &'static str {
        match self {
            Self::NoTunnel => "upstream_unavailable",
            Self::TunnelConflict => "conflict",
            Self::StreamLimit => "stream_limit",
            Self::DuplicateStream => "conflict",
            Self::EpochChanged => "conflict",
            Self::BodyTooLarge => "body_too_large",
        }
    }
}

/// Why an admitted call could not continue talking to its tunnel.
///
/// Once a call fails this way it is terminal: the caller must not retry the
/// same stream, because a partially written request cannot be resumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayCallError {
    /// The tunnel is gone or its outbound queue is full. A full queue is not
    /// recoverable by retrying on the same connection: the tunnel is aborted
    /// instead, because dropping a control frame — a cancel in particular —
    /// would strand the agent side.
    TunnelUnavailable,
    /// A frame could not be encoded within the frozen protocol bounds.
    Protocol(RelayProtocolError),
}

impl RelayCallError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TunnelUnavailable => "upstream_unavailable",
            Self::Protocol(error) => error.code(),
        }
    }
}
