use hosttest::cmdline::{self, BootParams, Console, COMMAND_LINE_SIZE};

fn value(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("parameter value is utf-8")
}

#[test]
fn defaults_are_sane() {
    let report = cmdline::parse(b"");
    assert_eq!(report.params, BootParams::new());
    assert_eq!(report.params.console, Console::Both);
    assert_eq!(report.params.loglevel, 7);
    assert!(!report.params.selftest);
    assert!(report.params.init.is_none());
    assert!(report.params.root.is_none());
    assert!(!report.truncated);
    assert_eq!(report.unknown, 0);
    assert_eq!(report.invalid, 0);
}

#[test]
fn parses_all_supported_parameters() {
    let report = cmdline::parse(
        b"init=/sbin/init root=/dev/hda1 console=serial loglevel=4 selftest=1",
    );
    assert_eq!(value(report.params.init.unwrap().as_bytes()), "/sbin/init");
    assert_eq!(value(report.params.root.unwrap().as_bytes()), "/dev/hda1");
    assert_eq!(report.params.console, Console::Serial);
    assert_eq!(report.params.loglevel, 4);
    assert!(report.params.selftest);
    assert_eq!(report.unknown, 0);
    assert_eq!(report.invalid, 0);
}

#[test]
fn accepts_whitespace_and_last_value_wins() {
    let report = cmdline::parse(
        b"  root=/dev/hda1\tconsole=vga\nlogevel=2\r console=both  root=/dev/hdb2  ",
    );
    assert_eq!(value(report.params.root.unwrap().as_bytes()), "/dev/hdb2");
    assert_eq!(report.params.console, Console::Both);
    assert_eq!(report.params.loglevel, 7);
    assert_eq!(report.unknown, 1);
}

#[test]
fn handles_quotes_and_escapes() {
    let report = cmdline::parse(b"root=\"/dev/disk by name/root\" init=\"/sbin/a\\\"b\"");
    assert_eq!(
        value(report.params.root.unwrap().as_bytes()),
        "/dev/disk by name/root"
    );
    assert_eq!(value(report.params.init.unwrap().as_bytes()), "/sbin/a\"b");
    assert_eq!(report.unknown, 0);
    assert_eq!(report.invalid, 0);
}

#[test]
fn accepts_escaped_whitespace_outside_quotes() {
    let report = cmdline::parse(b"root=/dev/disk\\ by\\ name/root console=vga");
    assert_eq!(
        value(report.params.root.unwrap().as_bytes()),
        "/dev/disk by name/root"
    );
    assert_eq!(report.params.console, Console::Vga);
    assert_eq!(report.unknown, 0);
    assert_eq!(report.invalid, 0);
}

#[test]
fn keeps_quoted_empty_values_and_rejects_them_by_policy() {
    let report = cmdline::parse(b"root=\"\" init=\"\"");
    assert!(report.params.root.is_none());
    assert!(report.params.init.is_none());
    assert_eq!(report.invalid, 2);
}

#[test]
fn stops_processing_after_double_dash() {
    let report = cmdline::parse(b"root=/dev/hda1 -- init=/bin/sh console=vga");
    assert_eq!(value(report.params.root.unwrap().as_bytes()), "/dev/hda1");
    assert!(report.params.init.is_none());
    assert_eq!(report.params.console, Console::Both);
    assert_eq!(report.unknown, 0);
    assert_eq!(report.invalid, 0);
}

#[test]
fn counts_unknown_and_malformed_tokens() {
    let report = cmdline::parse(b"quiet root=/dev/hda1 weird=1 init=relative loglevel=9");
    assert_eq!(report.unknown, 2);
    assert_eq!(report.invalid, 2);
    assert!(report.params.init.is_none());
    assert_eq!(report.params.loglevel, 7);
}

#[test]
fn validates_console_values() {
    for (input, expected) in [
        (&b"console=serial"[..], Console::Serial),
        (&b"console=vga"[..], Console::Vga),
        (&b"console=both"[..], Console::Both),
    ] {
        let report = cmdline::parse(input);
        assert_eq!(report.params.console, expected);
        assert_eq!(report.invalid, 0);
    }

    for input in [&b"console="[..], b"console=UART", b"console=serial0"] {
        let report = cmdline::parse(input);
        assert_eq!(report.params.console, Console::Both);
        assert_eq!(report.invalid, 1);
    }
}

#[test]
fn validates_loglevel_range() {
    for level in 0..=7u8 {
        let input = format!("loglevel={level}");
        let report = cmdline::parse(input.as_bytes());
        assert_eq!(report.params.loglevel, level);
        assert_eq!(report.invalid, 0);
    }

    for input in [&b"loglevel=8"[..], b"loglevel=255", b"loglevel=", b"loglevel=7x"] {
        let report = cmdline::parse(input);
        assert_eq!(report.params.loglevel, 7);
        assert_eq!(report.invalid, 1);
    }
}

#[test]
fn validates_selftest_and_init_paths() {
    let report = cmdline::parse(b"selftest=0 init=/");
    assert!(!report.params.selftest);
    assert_eq!(value(report.params.init.unwrap().as_bytes()), "/");
    assert_eq!(report.invalid, 0);

    let report = cmdline::parse(b"selftest=2 init=relative/path");
    assert!(!report.params.selftest);
    assert!(report.params.init.is_none());
    assert_eq!(report.invalid, 2);
}

#[test]
fn rejects_over_long_values() {
    let mut input = b"init=/".to_vec();
    input.extend(core::iter::repeat(b'a').take(300));
    let report = cmdline::parse(&input);
    assert!(report.params.init.is_none());
    assert_eq!(report.invalid, 1);
}

#[test]
fn reports_over_long_tokens_without_aborting() {
    let mut input = b"root=/dev/hda1 ".to_vec();
    input.extend(core::iter::repeat(b'x').take(600));
    input.extend_from_slice(b" console=vga");
    let report = cmdline::parse(&input);
    assert_eq!(value(report.params.root.unwrap().as_bytes()), "/dev/hda1");
    assert!(report.truncated);
    assert_eq!(report.params.console, Console::Vga);
}

#[test]
fn init_copies_and_parses_the_boot_command_line() {
    let mut input = b"root=/dev/hda1 console=vga ".to_vec();
    input.extend(core::iter::repeat(b'x').take(COMMAND_LINE_SIZE + 64));
    let report = cmdline::init(&input);

    assert_eq!(report.stored_len, COMMAND_LINE_SIZE - 1);
    assert!(report.truncated);
    assert_eq!(value(report.params.root.unwrap().as_bytes()), "/dev/hda1");
    assert_eq!(report.params.console, Console::Vga);
    assert_eq!(cmdline::params(), report.params);
}
