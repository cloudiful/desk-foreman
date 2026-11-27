//! Shared nonproduction test database URL for the execution-environment
//! integration suites.

/// Read the dedicated nonproduction test database URL.
///
/// Panics when missing so an explicit `cargo test --test
/// execution_environments` without the variable fails instead of
/// silently passing.
pub fn test_database_url() -> String {
    std::env::var("DF_ENV_TEST_DATABASE_URL").expect(
        "DF_ENV_TEST_DATABASE_URL is required for execution-environment integration tests; \
         set it to a dedicated nonproduction PostgreSQL URL (never production)",
    )
}
