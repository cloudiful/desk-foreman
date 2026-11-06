use crate::db::types::GitWorkspaceSyncRequest;

fn request() -> GitWorkspaceSyncRequest {
    serde_json::from_str(
        r#"{
            "remote_url": "https://broker:smcpw1.00000000-0000-4000-8000-000000000000.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa@broker.example.invalid/api/internal/code-projects/11111111-1111-4111-8111-111111111111/git",
            "branch": "main",
            "preserve_existing": false,
            "code_project_id": "11111111-1111-4111-8111-111111111111",
            "authorization_generation": "22222222-2222-4222-8222-222222222222"
        }"#,
    )
    .expect("request deserializes")
}

#[test]
fn request_debug_redacts_the_scoped_remote_url() {
    let request = request();
    let debug = format!("{request:?}");
    assert!(!debug.contains("smcpw1"));
    assert!(!debug.contains("broker.example.invalid"));
    assert!(debug.contains("[redacted]"));
    assert_eq!(
        request.code_project_id.to_string(),
        "11111111-1111-4111-8111-111111111111"
    );
    assert_eq!(
        request.authorization_generation.to_string(),
        "22222222-2222-4222-8222-222222222222"
    );
}

#[test]
fn request_accepts_optional_branch_and_preserves_existing_default() {
    let request: GitWorkspaceSyncRequest = serde_json::from_str(
        r#"{
            "remote_url": "https://example.invalid/private",
            "code_project_id": "11111111-1111-4111-8111-111111111111",
            "authorization_generation": "22222222-2222-4222-8222-222222222222"
        }"#,
    )
    .expect("request deserializes");
    assert!(request.branch.is_none());
    assert!(!request.preserve_existing);
}
