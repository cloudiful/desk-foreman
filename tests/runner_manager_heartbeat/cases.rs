//! Heartbeat persistence throttle cases (issue 1176 P1).

use chrono::{Duration, Utc};

use desk_foreman::{
    auth::hash_bearer_token,
    db::{queries, types::RunnerManagerAuthRecord},
};

use super::support::{
    FIND_AUTH_SQL, HEARTBEAT_WRITE_INTERVAL_SECS, HeartbeatFixture, ManagerRow, TOKEN, TOUCH_SQL,
    begin_transaction, cancel_backend, database_now, rollback_transaction, seed_row, select_row,
    wait_for_blocked_updates, wait_for_no_blocked_updates,
};

fn assert_online_after(row: &ManagerRow, before: &ManagerRow) {
    assert_eq!(row.status, "online");
    let refreshed = row.last_seen_at.expect("heartbeat timestamp");
    let previous = before.last_seen_at.expect("previous heartbeat timestamp");
    assert!(
        refreshed > previous,
        "stale heartbeat must advance: {refreshed} <= {previous}"
    );
}

#[tokio::test]
async fn initial_pending_contact_refreshes_and_goes_live() {
    let fixture = HeartbeatFixture::setup().await;
    let before = fixture.seed(true, "pending", None).await;
    assert!(before.last_seen_at.is_none());

    let (status, body) = fixture.config().await;
    assert_eq!(status, 200, "config response: {body}");
    assert_eq!(body["runner_manager_id"], fixture.manager_id);

    let after = fixture.row().await;
    assert_eq!(after.status, "online");
    assert!(after.last_seen_at.is_some());
    assert!(
        queries::find_enabled_runner_manager(fixture.pool())
            .await
            .expect("find live manager")
            .is_some_and(|manager| manager.runner_manager_id == fixture.manager_id)
    );

    let wrapper = fixture
        .record_by_token()
        .await
        .expect("compatibility wrapper keeps the public record shape");
    assert_eq!(wrapper.runner_manager_id, fixture.manager_id);

    let pending_fresh = fixture
        .seed_at(
            true,
            "pending",
            Some(Utc::now() - Duration::seconds(1)),
            Utc::now() - Duration::seconds(1),
        )
        .await;
    let (status, body) = fixture.config().await;
    assert_eq!(status, 200, "config response: {body}");
    assert_online_after(&fixture.row().await, &pending_fresh);

    fixture.teardown().await;
}

#[tokio::test]
async fn fresh_repeated_auth_keeps_timestamps_and_skips_update() {
    let fixture = HeartbeatFixture::setup().await;
    fixture.install_touch_log().await;
    let before = fixture
        .seed_at(
            true,
            "online",
            Some(Utc::now() - Duration::seconds(1)),
            Utc::now() - Duration::seconds(1),
        )
        .await;
    fixture.clear_touch_log().await;

    for _ in 0..2 {
        let (status, body) = fixture.config().await;
        assert_eq!(status, 200, "config response: {body}");
    }

    let after = fixture.row().await;
    assert_eq!(after.last_seen_at, before.last_seen_at);
    assert_eq!(after.updated_at, before.updated_at);
    assert_eq!(fixture.touch_count().await, 0, "fresh auth must not UPDATE");

    fixture.teardown().await;
}

#[tokio::test]
async fn fresh_auth_does_not_wait_for_a_row_lock() {
    let fixture = HeartbeatFixture::setup().await;
    fixture.install_touch_log().await;
    fixture
        .seed_at(
            true,
            "online",
            Some(Utc::now() - Duration::seconds(1)),
            Utc::now() - Duration::seconds(1),
        )
        .await;
    fixture.clear_touch_log().await;

    let lock = fixture.hold_row_lock().await;
    let answer = tokio::time::timeout(std::time::Duration::from_secs(3), fixture.config()).await;
    HeartbeatFixture::release_row_lock(lock).await;

    let (status, body) = answer.expect("a fresh auth request must not attempt the locked UPDATE");
    assert_eq!(status, 200, "config response: {body}");
    assert_eq!(fixture.touch_count().await, 0);

    fixture.teardown().await;
}

#[tokio::test]
async fn stale_auth_refreshes_and_stays_live() {
    let fixture = HeartbeatFixture::setup().await;
    let before = fixture.seed(true, "online", Some(11.0)).await;

    let (status, body) = fixture.config().await;
    assert_eq!(status, 200, "config response: {body}");

    let after = fixture.row().await;
    assert_online_after(&after, &before);
    assert!(
        queries::find_enabled_runner_manager(fixture.pool())
            .await
            .expect("find live manager")
            .is_some_and(|manager| manager.runner_manager_id == fixture.manager_id)
    );

    fixture.teardown().await;
}

#[tokio::test]
async fn heartbeat_due_window_starts_at_ten_seconds_inclusively() {
    let fixture = HeartbeatFixture::setup().await;
    let mut conn = fixture
        .pool()
        .acquire()
        .await
        .expect("acquire clock connection");
    let now = database_now(&mut conn).await;
    drop(conn);

    fixture
        .seed_at(
            true,
            "online",
            Some(now - Duration::milliseconds(9_500)),
            now,
        )
        .await;
    let fresh = fixture.auth().await.expect("auth row");
    assert!(!fresh.heartbeat_due, "9.5s is inside the ten-second window");

    fixture
        .seed_at(true, "online", Some(now - Duration::seconds(10)), now)
        .await;
    let boundary = fixture.auth().await.expect("auth row");
    assert!(
        boundary.heartbeat_due,
        "the ten-second boundary is due (inclusive expiry)"
    );

    fixture.teardown().await;
}

#[tokio::test]
async fn exact_transaction_clock_equality_is_due_in_both_predicates() {
    let fixture = HeartbeatFixture::setup().await;
    let mut conn = fixture.pool().acquire().await.expect("acquire transaction");
    begin_transaction(&mut conn).await;
    let frozen = database_now(&mut conn).await;
    let expected = frozen - Duration::seconds(HEARTBEAT_WRITE_INTERVAL_SECS);
    let boundary = seed_row(
        &mut conn,
        fixture.manager_id,
        true,
        "online",
        Some(expected),
        frozen,
    )
    .await;
    assert_eq!(boundary.last_seen_at, Some(expected));

    let auth: RunnerManagerAuthRecord = sqlx::query_as(FIND_AUTH_SQL)
        .bind(hash_bearer_token(TOKEN))
        .bind(HEARTBEAT_WRITE_INTERVAL_SECS)
        .fetch_one(&mut *conn)
        .await
        .expect("auth select at the exact boundary");
    assert!(
        auth.heartbeat_due,
        "SELECT due must include exact equality against the transaction clock"
    );

    let first = sqlx::query(TOUCH_SQL)
        .bind(fixture.manager_id)
        .bind(HEARTBEAT_WRITE_INTERVAL_SECS)
        .execute(&mut *conn)
        .await
        .expect("guarded touch at the exact boundary");
    assert_eq!(
        first.rows_affected(),
        1,
        "UPDATE guard must include exact equality against the transaction clock"
    );
    let touched = select_row(&mut conn, fixture.manager_id).await;
    assert_eq!(touched.status, "online");
    assert_eq!(touched.last_seen_at, Some(frozen));
    assert_eq!(touched.updated_at, frozen);

    let second = sqlx::query(TOUCH_SQL)
        .bind(fixture.manager_id)
        .bind(HEARTBEAT_WRITE_INTERVAL_SECS)
        .execute(&mut *conn)
        .await
        .expect("second guarded touch");
    assert_eq!(second.rows_affected(), 0, "the refreshed row is fresh");

    rollback_transaction(&mut conn).await;
    drop(conn);
    fixture.teardown().await;
}

#[tokio::test]
async fn disabled_and_unknown_tokens_reject_without_touching() {
    let fixture = HeartbeatFixture::setup().await;
    let before = fixture
        .seed_at(
            false,
            "online",
            Some(Utc::now() - Duration::seconds(20)),
            Utc::now() - Duration::seconds(20),
        )
        .await;

    let (status, _) = fixture.config().await;
    assert_eq!(status, 403);
    let after = fixture.row().await;
    assert!(!after.enabled);
    assert_eq!(after.last_seen_at, before.last_seen_at);
    assert_eq!(after.updated_at, before.updated_at);

    let (status, _) = fixture.config_with_token("dfm_unknown_token").await;
    assert_eq!(status, 404);

    let disabled = fixture
        .seed_at(
            false,
            "online",
            Some(Utc::now() - Duration::seconds(1)),
            Utc::now() - Duration::seconds(1),
        )
        .await;
    let (status, _) = fixture.config().await;
    assert_eq!(status, 403);
    assert_eq!(fixture.row().await.updated_at, disabled.updated_at);

    fixture.teardown().await;
}

#[tokio::test]
async fn competing_stale_callers_yield_one_effective_refresh() {
    let fixture = HeartbeatFixture::setup().await;
    fixture.install_touch_log().await;
    let before = fixture.seed(true, "online", Some(11.0)).await;
    fixture.clear_touch_log().await;

    let mut lock = fixture.hold_row_lock().await;
    let handles: Vec<_> = (0..3).map(|_| fixture.spawn_config_request()).collect();
    let blocked = wait_for_blocked_updates(&mut lock, 3).await;
    assert_eq!(
        blocked.len(),
        3,
        "every caller must have observed stale data and reached the guarded UPDATE"
    );
    HeartbeatFixture::release_row_lock(lock).await;

    for handle in handles {
        let (status, body) = handle.await.expect("config request task");
        assert_eq!(status, 200, "config response: {body}");
    }

    let after = fixture.row().await;
    assert_online_after(&after, &before);
    assert_eq!(after.last_seen_at, Some(after.updated_at));
    assert_eq!(
        fixture.touch_count().await,
        1,
        "only the first competing UPDATE may advance the heartbeat"
    );

    fixture.teardown().await;
}

#[tokio::test]
async fn direct_touch_guard_skips_fresh_rows() {
    let fixture = HeartbeatFixture::setup().await;
    fixture.install_touch_log().await;
    let fresh = fixture
        .seed_at(
            true,
            "online",
            Some(Utc::now() - Duration::seconds(1)),
            Utc::now() - Duration::seconds(1),
        )
        .await;
    fixture.clear_touch_log().await;

    queries::touch_runner_manager(fixture.pool(), fixture.manager_id)
        .await
        .expect("guarded touch");
    let unchanged = fixture.row().await;
    assert_eq!(unchanged.last_seen_at, fresh.last_seen_at);
    assert_eq!(unchanged.updated_at, fresh.updated_at);
    assert_eq!(fixture.touch_count().await, 0);

    let stale = fixture.seed(true, "online", Some(11.0)).await;
    fixture.clear_touch_log().await;
    queries::touch_runner_manager(fixture.pool(), fixture.manager_id)
        .await
        .expect("guarded touch");
    let refreshed = fixture.row().await;
    assert_online_after(&refreshed, &stale);
    assert_eq!(fixture.touch_count().await, 1);

    fixture.teardown().await;
}

#[tokio::test]
async fn failed_write_can_be_retried_without_poisoning() {
    let fixture = HeartbeatFixture::setup().await;
    fixture.install_touch_failure().await;
    let before = fixture.seed(true, "online", Some(11.0)).await;

    fixture.set_touch_failure(true).await;
    let (status, _) = fixture.config().await;
    assert!(status >= 500, "failed touch must surface: {status}");
    let failed = fixture.row().await;
    assert_eq!(failed.last_seen_at, before.last_seen_at);
    assert_eq!(failed.updated_at, before.updated_at);

    fixture.set_touch_failure(false).await;
    let (status, body) = fixture.config().await;
    assert_eq!(status, 200, "config response: {body}");
    let retried = fixture.row().await;
    assert_online_after(&retried, &before);

    fixture.teardown().await;
}

#[tokio::test]
async fn pending_cancelled_write_leaves_no_remembered_freshness() {
    let fixture = HeartbeatFixture::setup().await;
    let before = fixture.seed(true, "online", Some(11.0)).await;

    let mut lock = fixture.hold_row_lock().await;
    let handle = fixture.spawn_config_request();
    let blocked = wait_for_blocked_updates(&mut lock, 1).await;
    assert_eq!(blocked.len(), 1);
    let cancelled = cancel_backend(&mut lock, blocked[0]).await;
    assert!(cancelled, "the pending guarded UPDATE must be cancellable");
    wait_for_no_blocked_updates(&mut lock).await;
    HeartbeatFixture::release_row_lock(lock).await;

    let (status, _) = handle.await.expect("config request task");
    assert!(
        status >= 500,
        "cancelled pending write must surface: {status}"
    );
    let after_cancel = fixture.row().await;
    assert_eq!(after_cancel.last_seen_at, before.last_seen_at);
    assert_eq!(after_cancel.updated_at, before.updated_at);
    assert!(
        fixture.auth().await.expect("auth row").heartbeat_due,
        "a cancelled write must leave the database as the only freshness source"
    );

    let (status, body) = fixture.config().await;
    assert_eq!(status, 200, "config response: {body}");
    assert_online_after(&fixture.row().await, &before);

    fixture.teardown().await;
}

#[tokio::test]
async fn freshness_is_read_from_the_database_every_request() {
    let fixture = HeartbeatFixture::setup().await;
    let before = fixture.seed(true, "online", Some(11.0)).await;

    let externally_stale = fixture.seed(true, "online", Some(20.0)).await;
    let auth = fixture.auth().await.expect("auth row");
    assert!(auth.heartbeat_due);
    let (status, body) = fixture.config().await;
    assert_eq!(status, 200, "config response: {body}");
    let after = fixture.row().await;
    assert_online_after(&after, &before);
    assert!(
        after.last_seen_at.expect("timestamp") > externally_stale.last_seen_at.expect("timestamp")
    );

    let fresh = fixture
        .seed_at(
            true,
            "online",
            Some(Utc::now() - Duration::seconds(1)),
            Utc::now() - Duration::seconds(1),
        )
        .await;
    assert!(!fixture.auth().await.expect("auth row").heartbeat_due);
    assert_eq!(fixture.row().await.updated_at, fresh.updated_at);

    fixture.teardown().await;
}
