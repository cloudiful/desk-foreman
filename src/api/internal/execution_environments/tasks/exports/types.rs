use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::api::validation::validate_non_blank;

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct ScheduleNativeTaskExportRequest {
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub operation_token: String,
    pub stop_job_id: Uuid,
    #[validate(length(min = 1))]
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, ToSchema)]
pub struct NativeTaskExportDescriptor {
    pub task_id: Uuid,
    pub job_id: Uuid,
    pub stop_job_id: Uuid,
    pub state: String,
    pub selection_sha256: String,
    pub output_manifest_sha256: Option<String>,
}

impl From<crate::db::execution_environments::tasks::exports::NativeTaskExportDescriptor>
    for NativeTaskExportDescriptor
{
    fn from(
        value: crate::db::execution_environments::tasks::exports::NativeTaskExportDescriptor,
    ) -> Self {
        Self {
            task_id: value.task_id,
            job_id: value.job_id,
            stop_job_id: value.stop_job_id,
            state: value.state,
            selection_sha256: value.selection_sha256,
            output_manifest_sha256: value.output_manifest_sha256,
        }
    }
}
