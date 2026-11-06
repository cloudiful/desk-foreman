use super::clone_volume::{GitVolumeError, GitVolumePlan, is_missing_volume_error};
use std::time::Duration;
use uuid::Uuid;

#[test]
fn clone_volume_names_and_option_injection_are_closed() {
    let plan = GitVolumePlan::generate(
        Uuid::new_v4(),
        Uuid::new_v4(),
        4096,
        1000,
        1000,
        Duration::from_secs(10),
    )
    .expect("volume");
    assert!(plan.name.starts_with("dfgit-"));
    let args = plan.create_args("docker").expect("create");
    assert!(args.iter().any(|arg| arg.starts_with("o=size=")));
    assert!(is_missing_volume_error(
        "Error response from daemon: no such volume: abc",
        "abc"
    ));
    assert!(!is_missing_volume_error("no such volume: other", "abc"));
    assert_eq!(
        GitVolumePlan::generate(
            Uuid::new_v4(),
            Uuid::new_v4(),
            0,
            1000,
            1000,
            Duration::from_secs(10)
        ),
        Err(GitVolumeError::Limits)
    );
}
