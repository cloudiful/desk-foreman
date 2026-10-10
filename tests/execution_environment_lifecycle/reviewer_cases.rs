//! Reviewer-authored independent probes for issue 742 final audit.
//!
//! These cases are not executor coverage: each one drives the real internal
//! HTTP surface plus the P2 manager transport against an isolated
//! nonproduction PostgreSQL and asserts a contract the issue plan states
//! directly, so a regression surfaces as a failing test rather than as a
//! claim in a note.
//!
//! Both cases below reproduced the confirmed P2 defect in
//! `src/db/execution_environments/lifecycle/stop.rs`: `latest_stop` was chosen
//! from every stop row of the environment regardless of `target_generation`,
//! so a stop that already succeeded in an earlier generation permanently
//! short-circuited a later admission. The production repair scopes that
//! selection to the current candidate and bound manager; these probes pin the
//! repaired behavior, the repeated multi-cycle loop, and the per-candidate
//! in-flight/replay guards the repair must not weaken.

use super::helpers::{create_app_with_token, test_state};
use super::http::spawn_http;
use super::requests::{provision, replace, status, stop, wire_job_id};
use super::support::{
    LifecycleDb, bring_manager_online, create_environment, create_manager, ensure_ack, poll_job,
    report_ok, stop_ack,
};

/// Drive one environment all the way to a second-generation `ready`, so a stop
/// can be issued against the current authorization generation again.
async fn reach_second_generation_ready(
    base: &str,
    app_token: &str,
    mgr_token: &str,
    env: uuid::Uuid,
) {
    provision(base, app_token, env, 0, "rv-prov-1").await;
    let job = poll_job(base, mgr_token).await;
    report_ok(
        base,
        mgr_token,
        job["job_id"].as_str().unwrap(),
        ensure_ack(env, 0),
    )
    .await;
    let (_, desc) = status(base, app_token, env).await;
    assert_eq!(desc["state"], "ready", "generation 0 must be ready");

    let (_, desc) = stop(base, app_token, env, 0, "rv-stop-1").await;
    assert_eq!(desc["state"], "stopping", "first stop must be admitted");
    let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();
    let job = poll_job(base, mgr_token).await;
    assert_eq!(job["kind"], "stop_execution_environment");
    report_ok(
        base,
        mgr_token,
        &wire_job_id(&stop_job),
        stop_ack(env, 0, "exited"),
    )
    .await;
    let (_, desc) = status(base, app_token, env).await;
    assert_eq!(desc["state"], "replaced", "proof must be recorded");

    let (code, desc) = replace(base, app_token, env, 1, 0, &stop_job, "rv-repl-1").await;
    assert_eq!(code, 200, "replace must be admitted: {desc}");
    let job = poll_job(base, mgr_token).await;
    report_ok(
        base,
        mgr_token,
        job["job_id"].as_str().unwrap(),
        ensure_ack(env, 1),
    )
    .await;
    let (_, desc) = status(base, app_token, env).await;
    assert_eq!(desc["state"], "ready", "generation 1 must be ready");
    assert_eq!(desc["generation"], 1);
}

/// The plan closes the loop "stop proof -> replacement under application
/// authorization"; a replacement must not permanently disable stopping. A stop
/// issued after a completed replacement, with a fresh operation token and the
/// current authorization generation, has to be admitted and dispatched to the
/// bound manager.
#[tokio::test]
async fn stop_after_replacement_is_admitted_with_a_fresh_token() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "rv-restop-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "rv-restop-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "rv-restop-user").await;

    reach_second_generation_ready(&base, &app_token, &mgr_token, env).await;

    // Second stop at the current authorization generation with a fresh token.
    let (code, desc) = stop(&base, &app_token, env, 1, "rv-stop-2").await;
    assert_eq!(code, 200, "stop after replacement must be admitted: {desc}");
    assert_eq!(
        desc["state"], "stopping",
        "state must enter stopping: {desc}"
    );
    assert_eq!(
        desc["operation"]["kind"], "stop",
        "a stop operation must be dispatched: {desc}"
    );
    let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();

    let job = poll_job(&base, &mgr_token).await;
    assert_eq!(job["kind"], "stop_execution_environment");
    assert_eq!(
        job["payload"]["owner"]["ExecutionEnvironment"]["generation"], 1,
        "the second stop must target the new candidate: {job}"
    );
    report_ok(
        &base,
        &mgr_token,
        &wire_job_id(&stop_job),
        stop_ack(env, 1, "exited"),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced", "second proof must be recorded");

    handle.abort();
    db.teardown().await;
}

/// A second replacement after a second stop/replace cycle must also be
/// admitted with the new proof, proving the proof is consumed exactly once per
/// stop rather than permanently exhausting the lifecycle.
#[tokio::test]
async fn replacement_cycle_repeats_after_a_second_confirmed_stop() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "rv-cycle-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "rv-cycle-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "rv-cycle-user").await;

    reach_second_generation_ready(&base, &app_token, &mgr_token, env).await;

    let (code, desc) = stop(&base, &app_token, env, 1, "rv-cycle-stop-2").await;
    assert_eq!(code, 200, "second stop must be admitted: {desc}");
    let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();
    let job = poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        &wire_job_id(&stop_job),
        stop_ack(env, 1, "dead"),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced", "second proof must be recorded");
    assert_eq!(desc["generation"], 2, "second stop increments once");

    let (code, desc) = replace(&base, &app_token, env, 2, 1, &stop_job, "rv-cycle-repl-2").await;
    assert_eq!(code, 200, "second replace must be admitted: {desc}");
    assert_eq!(desc["state"], "starting");
    assert_eq!(desc["generation"], 2);

    handle.abort();
    db.teardown().await;
}

/// The repaired per-candidate selection must not weaken the guards for the
/// *current* candidate: an in-flight stop still refuses a second intent, a
/// confirmed-but-unreconciled stop still replays idempotently without a new
/// durable row, and a reconciled `replaced` environment still refuses a stop
/// until it is replaced again.
#[tokio::test]
async fn current_candidate_stop_guards_survive_the_repair() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "rv-guard-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "rv-guard-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "rv-guard-user").await;

    provision(&base, &app_token, env, 0, "rv-guard-prov").await;
    let job = poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        ensure_ack(env, 0),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");

    let (code, desc) = stop(&base, &app_token, env, 0, "rv-guard-a").await;
    assert_eq!(code, 200, "first stop must be admitted: {desc}");
    let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();

    // In flight: a different token must not admit a second stop.
    let (code, body) = stop(&base, &app_token, env, 0, "rv-guard-b").await;
    assert_eq!(
        code, 409,
        "an in-flight stop must refuse a second intent: {body}"
    );

    let job = poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        &wire_job_id(&stop_job),
        stop_ack(env, 0, "exited"),
    )
    .await;

    // Confirmed but not yet reconciled: a different token at the *current*
    // authorization generation replays the same operation instead of creating
    // a second durable stop row.
    let (code, desc) = stop(&base, &app_token, env, 1, "rv-guard-c").await;
    assert_eq!(code, 200, "an unreconciled stop replays: {desc}");
    assert_eq!(
        desc["operation"]["job_id"].as_str().unwrap(),
        stop_job,
        "the replay must reference the existing stop operation: {desc}"
    );
    let stop_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_jobs \
         WHERE environment_id = $1 AND job_kind = 'stop'",
    )
    .bind(env)
    .fetch_one(&db.pool)
    .await
    .expect("count stop rows");
    assert_eq!(
        stop_rows, 1,
        "an unreconciled replay must not add a stop row"
    );

    // Reconciled: the candidate is durably stopped, so a stop refuses until a
    // replacement advances the candidate again.
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");
    let (code, body) = stop(&base, &app_token, env, 1, "rv-guard-d").await;
    assert_eq!(
        code, 409,
        "a replaced environment must refuse a stop before replacement: {body}"
    );

    handle.abort();
    db.teardown().await;
}

/// Three consecutive stop/replace cycles on one environment: every cycle must
/// dispatch fresh ensure and stop operations for the current candidate, keep
/// the authorization generation and container candidate strictly increasing,
/// consume each proof exactly once, and never lose the original manager
/// binding.
#[tokio::test]
async fn three_repeated_stop_replace_cycles_are_monotonic() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "rv-multi-app").await;
    let (manager_id, mgr_token) = create_manager(&db.pool, "rv-multi-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "rv-multi-user").await;

    // Cycle 0 provisions; later cycles start from the recorded generation.
    let (code, desc) = provision(&base, &app_token, env, 0, "rv-multi-prov-0").await;
    assert_eq!(code, 200, "provision must be admitted: {desc}");

    let mut seen_stop_jobs: Vec<String> = Vec::new();
    for cycle in 0..3i64 {
        // Ensure for the current candidate.
        let job = poll_job(&base, &mgr_token).await;
        assert_eq!(job["kind"], "ensure_execution_environment", "cycle {cycle}");
        assert_eq!(
            job["payload"]["owner"]["ExecutionEnvironment"]["generation"], cycle,
            "ensure must target candidate {cycle}: {job}"
        );
        report_ok(
            &base,
            &mgr_token,
            job["job_id"].as_str().unwrap(),
            ensure_ack(env, cycle),
        )
        .await;
        let (_, desc) = status(&base, &app_token, env).await;
        assert_eq!(
            desc["state"], "ready",
            "cycle {cycle} must reach ready: {desc}"
        );
        assert_eq!(desc["generation"], cycle);

        // Stop at the current authorization generation.
        let (code, desc) = stop(
            &base,
            &app_token,
            env,
            cycle,
            &format!("rv-multi-stop-{cycle}"),
        )
        .await;
        assert_eq!(code, 200, "cycle {cycle} stop must be admitted: {desc}");
        assert_eq!(desc["state"], "stopping", "cycle {cycle}: {desc}");
        let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();
        assert!(
            !seen_stop_jobs.contains(&stop_job),
            "each cycle must dispatch a distinct stop operation"
        );
        seen_stop_jobs.push(stop_job.clone());
        assert_eq!(desc["generation"], cycle + 1, "one increment per stop");

        let job = poll_job(&base, &mgr_token).await;
        assert_eq!(job["kind"], "stop_execution_environment", "cycle {cycle}");
        assert_eq!(
            job["payload"]["owner"]["ExecutionEnvironment"]["generation"], cycle,
            "stop must target candidate {cycle}: {job}"
        );
        report_ok(
            &base,
            &mgr_token,
            &wire_job_id(&stop_job),
            stop_ack(env, cycle, "exited"),
        )
        .await;
        let (_, desc) = status(&base, &app_token, env).await;
        assert_eq!(
            desc["state"], "replaced",
            "cycle {cycle} must record a proof: {desc}"
        );

        // The proof is consumed exactly once: replaying it is refused.
        let (code, body) = replace(
            &base,
            &app_token,
            env,
            cycle + 1,
            cycle,
            &stop_job,
            &format!("rv-multi-repl-{cycle}"),
        )
        .await;
        assert_eq!(code, 200, "cycle {cycle} replace must be admitted: {body}");
        let (code, body) = replace(
            &base,
            &app_token,
            env,
            cycle + 1,
            cycle + 1,
            &stop_job,
            &format!("rv-multi-repl-{cycle}-again"),
        )
        .await;
        assert_eq!(
            code, 409,
            "cycle {cycle} proof must be consumed once: {body}"
        );
    }

    // Affinity and candidate survived every cycle; the barrier the manager
    // enforces never sees a rewind.
    let (bound, candidate): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT assigned_manager_id, container_generation \
         FROM execution_environments WHERE environment_id = $1",
    )
    .bind(env)
    .fetch_one(&db.pool)
    .await
    .expect("read lifecycle metadata");
    assert_eq!(bound, Some(manager_id), "affinity must never change");
    assert_eq!(
        candidate,
        Some(3),
        "the final candidate must be generation 3"
    );

    // Three cycles admit three ensures and three stops; the third replacement
    // additionally admits the generation-3 ensure, which is still pending.
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT job_kind, state, target_generation \
         FROM execution_environment_jobs WHERE environment_id = $1 \
         ORDER BY created_at ASC, job_id ASC",
    )
    .bind(env)
    .fetch_all(&db.pool)
    .await
    .expect("read job rows");
    let stops: Vec<&(String, String, i64)> = rows.iter().filter(|r| r.0 == "stop").collect();
    let ensures: Vec<&(String, String, i64)> = rows.iter().filter(|r| r.0 == "ensure").collect();
    assert_eq!(stops.len(), 3, "one stop per cycle: {rows:?}");
    assert!(
        stops.iter().all(|r| r.1 == "succeeded"),
        "every stop proof must be confirmed: {rows:?}"
    );
    assert_eq!(
        stops.iter().map(|r| r.2).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "stops must target strictly increasing candidates: {rows:?}"
    );
    assert_eq!(
        ensures.len(),
        4,
        "three ensures plus one replacement: {rows:?}"
    );
    assert!(
        ensures.iter().all(|r| r.1 == "succeeded" || r.2 == 3),
        "only the final generation-3 ensure may stay pending: {rows:?}"
    );
    assert_eq!(
        ensures
            .iter()
            .filter(|r| r.1 == "succeeded")
            .map(|r| r.2)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
        "each proven candidate must have run an ensure exactly once: {rows:?}"
    );

    handle.abort();
    db.teardown().await;
}
