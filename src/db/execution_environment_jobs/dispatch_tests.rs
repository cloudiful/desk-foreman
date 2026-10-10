//! Unit tests for the durable wire namespace (issue 742 P2, `#[cfg(test)]`).

#[cfg(test)]
mod dispatch_tests {
    use super::super::dispatch::{
        ENV_JOB_ID_PREFIX, encode_environment_job_wire_id, is_environment_job_wire_id,
        parse_environment_job_wire_id,
    };
    use uuid::Uuid;

    #[test]
    fn wire_ids_use_the_reserved_namespace() {
        let id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let encoded = encode_environment_job_wire_id(id);
        assert!(encoded.starts_with(ENV_JOB_ID_PREFIX));
        assert!(is_environment_job_wire_id(&encoded));
        assert_eq!(parse_environment_job_wire_id(&encoded), Some(id));

        // Legacy plain UUID ids stay out of the durable namespace.
        assert!(!is_environment_job_wire_id(
            "11111111-1111-1111-1111-111111111111"
        ));
        // Malformed prefixed ids are still durable (and thus never legacy).
        assert!(is_environment_job_wire_id("envjob:not-a-uuid"));
        assert_eq!(parse_environment_job_wire_id("envjob:not-a-uuid"), None);
        assert_eq!(parse_environment_job_wire_id("envjob:"), None);
    }
}
