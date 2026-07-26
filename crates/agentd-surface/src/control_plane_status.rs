//! Maps control-plane port errors onto the project-wide HTTP status
//! convention: `Invalid` -> 400, `NotFound` -> 404, `Conflict` -> 409,
//! `Unavailable` -> 503. Only 503 is retryable by a client; every other
//! status is terminal. Collapsing variants onto one status is what made
//! transient database contention look like a permanent worker failure.

use agentd_core::ports::{TaskLeaseError, WorkerFleetError};
use axum::http::StatusCode;

/// The HTTP status a control-plane port error maps to. Implemented in this
/// crate for foreign port error types (legal: the trait is local).
pub trait ControlPlaneErrorStatus {
    fn http_status(&self) -> StatusCode;
}

impl ControlPlaneErrorStatus for WorkerFleetError {
    fn http_status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

impl ControlPlaneErrorStatus for TaskLeaseError {
    fn http_status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            // A rejected claim is an ownership/fencing conflict: the worker
            // must not retry it, it must re-acquire.
            Self::Conflict(_) | Self::Rejected { .. } => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ControlPlaneErrorStatus;
    use agentd_core::ports::{TaskLeaseError, TaskLeaseRejectionReason, WorkerFleetError};
    use axum::http::StatusCode;

    #[test]
    fn worker_fleet_error_variants_map_to_distinct_statuses() {
        assert_eq!(
            WorkerFleetError::Invalid("bad".into()).http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            WorkerFleetError::NotFound("gone".into()).http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            WorkerFleetError::Conflict("stale".into()).http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            WorkerFleetError::Unavailable("busy".into()).http_status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn task_lease_error_variants_map_to_distinct_statuses() {
        assert_eq!(
            TaskLeaseError::Invalid("bad".into()).http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            TaskLeaseError::NotFound("gone".into()).http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            TaskLeaseError::Conflict("fenced".into()).http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            TaskLeaseError::Rejected {
                reason: TaskLeaseRejectionReason::StaleFencingToken,
                message: "stale".into(),
            }
            .http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            TaskLeaseError::Unavailable("busy".into()).http_status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
