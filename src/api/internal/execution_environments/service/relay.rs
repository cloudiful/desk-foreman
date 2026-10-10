//! Opening and driving one relayed request (issue 749 P2b).
//!
//! The order here is the whole safety argument, so it is stated once:
//!
//! 1. bounds are checked before anything is opened;
//! 2. the owner-scoped row is read and the fences checked;
//! 3. the stream is admitted to the manager's tunnel;
//! 4. the row is read *again* and the stream's epoch is re-checked;
//! 5. only then is the opening frame written.
//!
//! Steps 3 and 4 together close the race where a stop or invalidation
//! commits between the caller's read and the tunnel write: either the epoch
//! moved, or the second read disagrees, and the stream is cancelled instead
//! of opened. The database is re-read rather than trusted because the
//! registry is a cache, not the lifecycle authority.

use std::sync::Arc;

use axum::body::Body;
use runner_protocol::service_relay::{OpenFrame, RelayId};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    error::AppError,
    runner::service_relay::{RelayAdmissionError, RelayCall, RelayRegistry, StreamRequest},
};

use super::{
    admission::{ServiceAdmission, admit_service_request, fence_outcome, refuse_admission},
    bounds::{ServiceRequestBounds, header_entry},
    response::relay_response,
};

/// Relay one already-bounded request to its environment's private agent.
pub async fn relay_service_request(
    registry: &Arc<RelayRegistry>,
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    presented_generation: i64,
    method: &str,
    bounds: ServiceRequestBounds,
    body: Body,
) -> Result<axum::response::Response, AppError> {
    let ServiceAdmission { manager_id } =
        admit_service_request(pool, application_id, environment_id, presented_generation).await?;
    let call = registry
        .open_stream(StreamRequest {
            stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
            environment_id,
            manager_id,
            declared_body_length: bounds.body_length,
        })
        .map_err(map_admission_error)?;

    // Re-check before the tunnel hears anything about this request.
    if let Err(error) = recheck_fence(
        pool,
        application_id,
        environment_id,
        presented_generation,
        &call,
    )
    .await
    {
        call.cancel();
        return Err(error);
    }

    call.declare_body_length(bounds.body_length)
        .map_err(|_| AppError::bad_request("declared body length exceeds the relay bound"))?;
    let allowed_headers = bounds
        .allowed_headers
        .iter()
        .map(|(name, value)| header_entry(name, value))
        .collect();
    let open = OpenFrame {
        stream_id: call.stream_id(),
        environment_id: RelayId::from_bytes(environment_id.into_bytes()),
        generation: presented_generation,
        method: method.to_string(),
        path: bounds.path,
        query: bounds.query,
        allowed_headers,
        body_length: bounds.body_length,
    };
    if call.send_open(&open).is_err() {
        call.cancel();
        return Err(AppError::service_unavailable(
            "the environment's manager relay is unavailable",
        ));
    }

    let Some(responses) = call.take_response() else {
        call.cancel();
        return Err(AppError::internal(anyhow::anyhow!(
            "relay stream already has a reader"
        )));
    };
    forward_request_body(call.clone(), body);
    relay_response(call, responses).await
}

/// Read the owner-scoped row once more and re-check the admitted epoch.
///
/// Both halves matter: the epoch catches a committed mutation that cancelled
/// this stream already, and the row catches one the registry has not seen.
async fn recheck_fence(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    presented_generation: i64,
    call: &RelayCall,
) -> Result<(), AppError> {
    if !call.epoch_is_current() {
        return Err(AppError::conflict(
            "execution environment stopped serving during admission",
        ));
    }
    let row = crate::db::execution_environments::find_execution_environment_for_service(
        pool,
        application_id,
        environment_id,
    )
    .await?;
    let Some(row) = row else {
        return Err(AppError::not_found("execution environment not found"));
    };
    let outcome = fence_outcome(&row, presented_generation);
    if outcome == super::admission::FenceOutcome::Servable {
        Ok(())
    } else {
        Err(refuse_admission(outcome))
    }
}

/// Forward the caller's body to the agent side, slice by slice.
///
/// A client that stops sending leaves the stream short of its declared
/// length, which the ticket refuses to close; the ticket's own drop then
/// cancels the stream, so a dropped body never reaches the agent described as
/// a complete request.
fn forward_request_body(call: RelayCall, body: Body) {
    tokio::spawn(async move {
        let mut stream = body.into_data_stream();
        while let Some(slice) = futures_util::StreamExt::next(&mut stream).await {
            let Ok(slice) = slice else {
                call.cancel();
                return;
            };
            if call.push_request_body(&slice).is_err() {
                call.cancel();
                return;
            }
        }
        if call.end_request().is_err() {
            call.cancel();
        }
    });
}

/// Map an admission refusal to its HTTP outcome.
fn map_admission_error(error: RelayAdmissionError) -> AppError {
    match error {
        RelayAdmissionError::NoTunnel => {
            AppError::service_unavailable("the environment's manager relay is unavailable")
        }
        RelayAdmissionError::TunnelConflict | RelayAdmissionError::StreamLimit => {
            AppError::conflict("the environment's manager relay cannot admit this request")
        }
        RelayAdmissionError::DuplicateStream => {
            AppError::conflict("the relay stream identifier is already in use")
        }
        RelayAdmissionError::EpochChanged => {
            AppError::conflict("execution environment stopped serving during admission")
        }
        RelayAdmissionError::BodyTooLarge => {
            AppError::bad_request("declared body length exceeds the relay bound")
        }
    }
}
