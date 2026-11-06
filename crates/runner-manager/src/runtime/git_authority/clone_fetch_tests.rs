use super::clone_fetch::{GitFetchError, parse_ls_remote, parse_object_batch, parse_tree_records};
use runner_protocol::GitObjectFormat;

#[test]
fn clone_fetch_parsers_are_bounded_and_exact() {
    let head = "a".repeat(40);
    assert_eq!(
        parse_ls_remote(format!("{head}\trefs/heads/main\n").as_bytes(), "main").expect("head"),
        (head.clone(), GitObjectFormat::Sha1)
    );
    assert_eq!(
        parse_ls_remote(format!("{head}\trefs/heads/main\n").as_bytes(), "other"),
        Err(GitFetchError::InvalidRemote)
    );
    let tree = format!("100644 blob {} 1\tfile\0", "b".repeat(40));
    assert_eq!(
        parse_tree_records(tree.as_bytes(), 1, 1)
            .expect("tree")
            .entries,
        1
    );
    assert!(parse_object_batch(b"not hex\n", 1, 1).is_err());
}
