use super::RetainedOutput;
use runner_protocol::RetainedSessionState;

#[test]
fn retained_output_cursor_is_repeatable_and_checked() {
    let mut output = RetainedOutput::new(42, 64);
    output.push(false, b"abc").unwrap();
    output.push(true, b"def").unwrap();
    let first = output.read(None, 4).unwrap();
    assert_eq!(first.stdout, b"abc");
    assert_eq!(first.stderr, b"d");
    assert!(!first.trimmed && first.truncated);
    assert_eq!(first, output.read(None, 4).unwrap());
    let rest = output.read(Some(&first.next_cursor), 4).unwrap();
    assert_eq!(rest.stderr, b"ef");
    assert!(!rest.truncated);
    assert_eq!(rest, output.read(Some(&first.next_cursor), 4).unwrap());
    for cursor in [
        "v1:2b:0:0",
        "v1:2a:4:0",
        "v1:2a:0:4",
        "v2:2a:0:0",
        "v1:2a:-1:0",
        "v1:2a:0:0:0",
        "v1:2a::0",
    ] {
        assert!(output.read(Some(cursor), 10).is_err(), "{cursor}");
    }
    assert!(output.read(None, 0).is_err());
    assert!(output.read(None, 1024 * 1024 + 1).is_err());
}

#[test]
fn retained_output_cursor_overflow_is_invalid() {
    let output = RetainedOutput::new(u64::MAX, 64);
    assert!(output.read(Some("v1:ffffffffffffffff:0:0"), 1).is_ok());
    assert!(output.read(Some("v1:ffffffffffffffff:1:0"), 1).is_err());
    for cursor in [
        "v1:10000000000000000:0:0",
        "v1:ffffffffffffffff:10000000000000000:0",
        "v1:ffffffffffffffff:0:10000000000000000",
        "v1:1:0:0",
        "v1:ffffffffffffffff:x:0",
    ] {
        assert!(
            format!("{:#}", output.read(Some(cursor), 1).unwrap_err())
                .contains("invalid output cursor"),
            "{cursor}"
        );
    }
}

#[test]
fn retained_output_eviction_and_binary_caps_are_exact() {
    let mut output = RetainedOutput::new(7, 4);
    output.push(false, "a€bc".as_bytes()).unwrap();
    output.push(true, &[255, 1, 2, 3, 4]).unwrap();
    let chunk = output.read(None, 3).unwrap();
    assert!(chunk.trimmed);
    assert_eq!(chunk.dropped_bytes, 3);
    assert_eq!(chunk.stdout, vec![0x82, 0xac, b'b']);
    assert_eq!(chunk.stdout_bytes, 6);
    assert_eq!(chunk.stderr_bytes, 5);
    let next = output.read(Some(&chunk.next_cursor), 8).unwrap();
    assert!(!next.trimmed);
    assert_eq!(next.stdout, b"c");
    assert_eq!(next.stderr, vec![1, 2, 3, 4]);
}

#[test]
fn retained_output_eof_requires_terminal_readers_and_delivery() {
    let mut output = RetainedOutput::new(1, 10);
    output.push(false, b"ab").unwrap();
    output.reader_complete(false);
    output.reader_complete(true);
    assert!(!output.read(None, 10).unwrap().eof);
    output
        .terminal(RetainedSessionState::TimedOut, None)
        .unwrap();
    output
        .terminal(RetainedSessionState::Exited, Some(0))
        .unwrap();
    assert_eq!(output.status().state, RetainedSessionState::TimedOut);
    assert_eq!(output.status().exit_code, None);
    let partial = output.read(None, 1).unwrap();
    assert!(partial.output_complete);
    assert!(!partial.eof);
    assert!(output.read(Some(&partial.next_cursor), 1).unwrap().eof);

    let mut draining = RetainedOutput::new(2, 10);
    draining
        .terminal(RetainedSessionState::Cancelled, None)
        .unwrap();
    draining.reader_complete(false);
    assert!(!draining.status().output_complete);
    assert!(!draining.read(None, 1).unwrap().eof);
    draining.reader_complete(true);
    assert!(draining.read(None, 1).unwrap().eof);
}
