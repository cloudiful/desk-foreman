//! Native Git Smart HTTP gateway integration test (issue 1180 P28).
//!
//! Runs a real `git clone`/`fetch`/`push` through the DF gateway against a
//! loopback `git http-backend` fixture backed by a temporary bare repository.
//! Requires `DF_GIT_NATIVE_TEST_DATABASE_URL` pointing at the dedicated
//! nonproduction issue database; the test creates and deletes only its own rows.

#[path = "git_gateway_native/support.rs"]
mod support;

use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD};
use support::{EXTERNAL_USER_ID, Fixture, run_git, run_ok};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_clone_fetch_and_push_through_gateway() {
    let fixture = Fixture::setup().await;
    let capability = mint(&fixture).await;
    let askpass = askpass_script(&fixture, &capability);
    let env = askpass_env(&askpass);

    let clone_dir = fixture.scratch.join("clone");
    let output = run_git(
        &[
            "-c",
            "credential.helper=",
            "clone",
            &fixture.remote_url("origin"),
            clone_dir.to_str().unwrap(),
        ],
        &fixture.scratch,
        &env,
    );
    assert!(
        output.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(clone_dir.join("README.md")).unwrap(),
        "hello from the fixture\n"
    );

    let config = std::fs::read_to_string(clone_dir.join(".git/config")).unwrap();
    assert!(
        !config.contains(&capability),
        "capability must never be persisted into .git/config"
    );
    assert!(
        !config.contains(&fixture.upstream_url),
        "upstream target must never be persisted into .git/config"
    );

    run_ok(
        "git",
        &["config", "user.email", "fixture@example.com"],
        &clone_dir,
        &env,
    );
    run_ok("git", &["config", "user.name", "Fixture"], &clone_dir, &env);
    std::fs::write(clone_dir.join("feature.txt"), "pushed through DF\n").unwrap();
    run_ok("git", &["add", "feature.txt"], &clone_dir, &env);
    run_ok("git", &["commit", "-m", "feature commit"], &clone_dir, &env);
    let head = run_git(&["rev-parse", "HEAD"], &clone_dir, &env);
    let head = String::from_utf8(head.stdout).unwrap().trim().to_string();
    let output = run_git(
        &["push", "origin", "HEAD:refs/heads/feature"],
        &clone_dir,
        &env,
    );
    assert!(
        output.status.success(),
        "git push failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pushed = run_git(
        &[
            "--git-dir",
            fixture.bare_repo.to_str().unwrap(),
            "rev-parse",
            "refs/heads/feature",
        ],
        &fixture.scratch,
        &env,
    );
    assert!(pushed.status.success(), "pushed ref missing from bare repo");
    assert_eq!(String::from_utf8(pushed.stdout).unwrap().trim(), head);

    let output = run_git(&["fetch", "origin"], &clone_dir, &env);
    assert!(
        output.status.success(),
        "git fetch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    fixture.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_rejects_route_auth_and_upstream_failures() {
    let fixture = Fixture::setup().await;
    let capability = mint(&fixture).await;
    let client = reqwest::Client::new();

    let info = |alias: &str, query: &str| {
        format!(
            "{}/git/{}/{}/info/refs{}",
            fixture.gateway_url, fixture.binding_id, alias, query
        )
    };

    let response = client.get(info("origin", "")).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let response = client
        .get(info("origin", "?service=git-bogus"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    let response = client
        .get(info("origin", "?service=git-upload-pack"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(
        response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .is_some_and(|value| value.to_str().unwrap().starts_with("Basic"))
    );

    let wrong_user = basic("not-df-git", &capability);
    let response = client
        .get(info("origin", "?service=git-upload-pack"))
        .header(reqwest::header::AUTHORIZATION, wrong_user)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    let mut tampered = capability.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    let response = client
        .get(info("origin", "?service=git-upload-pack"))
        .header(reqwest::header::AUTHORIZATION, basic("df-git", &tampered))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    let response = client
        .get(info("denied", "?service=git-upload-pack"))
        .header(reqwest::header::AUTHORIZATION, basic("df-git", &capability))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(
        response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .is_none(),
        "upstream 401 must not leak a WWW-Authenticate challenge"
    );
    let body = response.text().await.unwrap();
    assert!(!body.contains(&fixture.denied_url));
    assert!(!body.contains("upstream-pass"));

    let response = client
        .get(info("redirect", "?service=git-upload-pack"))
        .header(reqwest::header::AUTHORIZATION, basic("df-git", &capability))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    assert!(
        response.headers().get(reqwest::header::LOCATION).is_none(),
        "upstream redirect Location must not be forwarded"
    );
    let body = response.text().await.unwrap();
    assert!(!body.contains(&fixture.redirect_url));
    assert!(!body.contains("127.0.0.1:9"));

    let response = client
        .get(info("missing", "?service=git-upload-pack"))
        .header(reqwest::header::AUTHORIZATION, basic("df-git", &capability))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    let response = client
        .get(format!(
            "{}/git/{}/origin/other",
            fixture.gateway_url, fixture.binding_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    fixture.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_rejects_disabled_generation_cross_binding_and_read_only() {
    let fixture = Fixture::setup().await;
    let client = reqwest::Client::new();
    let info = |binding: i64, alias: &str| {
        format!(
            "{}/git/{}/{}/info/refs?service=git-upload-pack",
            fixture.gateway_url, binding, alias
        )
    };

    let upstreams = desk_foreman::db::queries::list_workspace_git_upstreams(
        &fixture.pool,
        fixture.binding_id,
        Some(50),
        Some(0),
    )
    .await
    .unwrap();
    let origin = upstreams
        .items
        .iter()
        .find(|upstream| upstream.remote_alias == "origin")
        .unwrap()
        .clone();

    let capability = mint(&fixture).await;
    desk_foreman::db::queries::set_workspace_git_upstream_enabled(
        &fixture.pool,
        fixture.binding_id,
        origin.upstream_id,
        false,
    )
    .await
    .unwrap();
    let response = client
        .get(info(fixture.binding_id, "origin"))
        .header(reqwest::header::AUTHORIZATION, basic("df-git", &capability))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "disabled alias must refuse before upstream"
    );

    desk_foreman::db::queries::set_workspace_git_upstream_enabled(
        &fixture.pool,
        fixture.binding_id,
        origin.upstream_id,
        true,
    )
    .await
    .unwrap();
    let fresh = mint(&fixture).await;
    let moved_target = format!(
        "{}/seed-work/.git",
        fixture.upstream_url.trim_end_matches("/fixture/repo.git")
    );
    let updated = desk_foreman::db::queries::update_workspace_git_upstream(
        &fixture.pool,
        Some(&support::credential_key()),
        fixture.binding_id,
        origin.upstream_id,
        desk_foreman::db::workspace_git_upstreams::WorkspaceGitUpstreamUpdate {
            remote_alias: None,
            target_url: Some(&moved_target),
            credential: None,
            clear_credential: false,
        },
    )
    .await
    .unwrap();
    assert!(updated.capability_generation > origin.capability_generation);
    let response = client
        .get(info(fixture.binding_id, "origin"))
        .header(reqwest::header::AUTHORIZATION, basic("df-git", &fresh))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "generation change must invalidate older capabilities"
    );

    let other_root = fixture.scratch.join("other-workspace");
    std::fs::create_dir_all(&other_root).unwrap();
    let other = desk_foreman::db::queries::create_workspace_binding(
        &fixture.pool,
        fixture.application_id,
        "other-user",
        "default",
        other_root.to_str().unwrap(),
        None,
        None,
    )
    .await
    .unwrap();
    desk_foreman::db::queries::create_workspace_git_upstream(
        &fixture.pool,
        Some(&support::credential_key()),
        other.workspace_binding_id,
        desk_foreman::db::workspace_git_upstreams::NewWorkspaceGitUpstream {
            remote_alias: "origin",
            target_url: &fixture.upstream_url,
            credential: None,
        },
    )
    .await
    .unwrap();
    let response = client
        .get(info(other.workspace_binding_id, "origin"))
        .header(reqwest::header::AUTHORIZATION, basic("df-git", &fresh))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "capability must not cross workspace bindings"
    );

    let resource_root = fixture.scratch.join("resource-workspace");
    std::fs::create_dir_all(&resource_root).unwrap();
    let resource = desk_foreman::db::queries::create_workspace_binding(
        &fixture.pool,
        fixture.application_id,
        EXTERNAL_USER_ID,
        "issue:42",
        resource_root.to_str().unwrap(),
        Some("issue"),
        Some("42"),
    )
    .await
    .unwrap();
    let response = client
        .post(format!(
            "{}/api/internal/workspace-bindings/{}/git-capability",
            fixture.gateway_url, resource.workspace_binding_id
        ))
        .bearer_auth(&fixture.token)
        .header("X-DF-External-User-Id", EXTERNAL_USER_ID)
        .json(&serde_json::json!({ "external_user_id": EXTERNAL_USER_ID }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "resource workspace without a lease must not receive write capability"
    );

    let (read_token, _) = desk_foreman::db::queries::create_application_token(
        &fixture.pool,
        &desk_foreman::db::types::CreateApplicationTokenRequest {
            application_id: fixture.application_id,
            token_name: "read-only-fixture".to_string(),
            expires_at: None,
            scopes: Some(vec![desk_foreman::policy::WORKSPACE_READ.to_string()]),
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: None,
        },
    )
    .await
    .unwrap();
    let (read_capability, _) = mint_with(&fixture, &read_token).await;
    let response = client
        .get(info(fixture.binding_id, "origin"))
        .header(
            reqwest::header::AUTHORIZATION,
            basic("df-git", &read_capability),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "read-only token may still fetch"
    );
    let response = client
        .post(format!(
            "{}/git/{}/origin/git-receive-pack",
            fixture.gateway_url, fixture.binding_id
        ))
        .header(
            reqwest::header::AUTHORIZATION,
            basic("df-git", &read_capability),
        )
        .header("Content-Type", "application/x-git-receive-pack-request")
        .body("0000")
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "read-only token must never receive write authority"
    );

    fixture.cleanup().await;
}

async fn mint(fixture: &Fixture) -> String {
    let (capability, gateway_base_url) = mint_with(fixture, &fixture.token).await;
    assert_eq!(gateway_base_url, fixture.gateway_url);
    capability
}

async fn mint_with(fixture: &Fixture, token: &str) -> (String, String) {
    let response = reqwest::Client::new()
        .post(fixture.capability_url())
        .bearer_auth(token)
        .header("X-DF-External-User-Id", EXTERNAL_USER_ID)
        .json(&serde_json::json!({ "external_user_id": EXTERNAL_USER_ID }))
        .send()
        .await
        .expect("capability request");
    assert!(
        response.status().is_success(),
        "capability mint failed: {}",
        response.status()
    );
    let value: serde_json::Value = response.json().await.unwrap();
    let capability = value["capability"].as_str().unwrap().to_string();
    assert!(capability.starts_with("dfgit1."));
    assert_eq!(value["gateway_base_url"], fixture.gateway_url);
    let aliases = value["upstreams"].as_array().unwrap();
    assert!(
        aliases
            .iter()
            .any(|alias| alias["remote_alias"] == "origin")
    );
    let rendered = value.to_string();
    assert!(!rendered.contains(&fixture.upstream_url));
    assert!(!rendered.contains("upstream-pass"));
    (
        capability,
        value["gateway_base_url"].as_str().unwrap().to_string(),
    )
}

fn askpass_script(fixture: &Fixture, capability: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = fixture.scratch.join("auth");
    std::fs::create_dir_all(&dir).unwrap();
    let capability_file = dir.join("capability");
    std::fs::write(&capability_file, capability).unwrap();
    let script = dir.join("askpass");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  Username*) printf '%s\\n' df-git ;;\n  Password*) cat {} ;;\n  *) exit 1 ;;\nesac\n",
            capability_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    script
}

fn askpass_env(script: &Path) -> Vec<(&str, String)> {
    vec![
        ("GIT_ASKPASS", script.to_str().unwrap().to_string()),
        ("GIT_TERMINAL_PROMPT", "0".to_string()),
    ]
}

fn basic(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    )
}
