//! Router on an ephemeral port for the execution-environment HTTP suites.

use desk_foreman::AppState;

/// Spawn the internal API router on an ephemeral port for HTTP tests.
pub async fn spawn_http(state: AppState) -> (String, tokio::task::JoinHandle<()>) {
    let router = desk_foreman::api::router().with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve test router");
    });
    (format!("http://{addr}"), handle)
}
