//! Mandatory named gate for native-task start/stop through the real DF router
//! and manager dispatch helper, with isolated PostgreSQL and a fake Docker CLI.

#[cfg(test)]
#[path = "native_task_execution/fake_docker.rs"]
mod fake_docker;

#[cfg(test)]
#[path = "native_task_execution/support.rs"]
pub(crate) mod support;

#[cfg(test)]
#[path = "native_task_execution/report.rs"]
mod report;

#[cfg(test)]
#[path = "native_task_execution/cases.rs"]
mod cases;
