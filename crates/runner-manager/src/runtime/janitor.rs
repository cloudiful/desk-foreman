use std::sync::Arc;
use std::time::Duration;

use tokio::time::interval;

use super::docker::DockerRunnerBackend;

pub fn spawn_janitor(backend: Arc<DockerRunnerBackend>, idle_ttl: Duration) {
    tokio::spawn(async move {
        let mut tick = interval(janitor_interval(idle_ttl));
        loop {
            tick.tick().await;
            backend.reclaim_idle_runners_inner().await;
        }
    });
}

fn janitor_interval(idle_ttl: Duration) -> Duration {
    if idle_ttl <= Duration::from_secs(60) {
        Duration::from_secs(5)
    } else {
        idle_ttl / 3
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::janitor_interval;

    #[test]
    fn janitor_interval_uses_one_third_of_idle_ttl() {
        assert_eq!(
            janitor_interval(Duration::from_secs(120)),
            Duration::from_secs(40)
        );
    }

    #[test]
    fn janitor_interval_falls_back_to_minimum_when_idle_ttl_short() {
        assert_eq!(
            janitor_interval(Duration::from_secs(30)),
            Duration::from_secs(5)
        );
    }
}
