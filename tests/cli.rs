//! End-to-end tests of the command-line interface (non-interactive paths).

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const SAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.md");

/// A `markterm` command with a clean colour environment.
fn markterm() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_markterm"));
    cmd.env_remove("NO_COLOR");
    cmd.stdin(Stdio::null());
    cmd
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("failed to run markterm")
}

fn has_esc(bytes: &[u8]) -> bool {
    bytes.contains(&0x1b)
}

fn tmp_file(name: &str, contents: &[u8]) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn run_with_stdin(cmd: &mut Command, input: &[u8]) -> Output {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("failed to spawn markterm");
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn piped_output_has_no_escape_codes_by_default() {
    let out = run(markterm().arg(SAMPLE));
    assert!(out.status.success());
    assert!(!out.stdout.is_empty());
    assert!(!has_esc(&out.stdout));
}

#[test]
fn color_always_emits_escape_codes_when_piped() {
    let out = run(markterm().args(["--color", "always", SAMPLE]));
    assert!(out.status.success());
    assert!(has_esc(&out.stdout));
}

#[test]
fn color_never_emits_no_escape_codes() {
    let out = run(markterm().args(["--color", "never", SAMPLE]));
    assert!(out.status.success());
    assert!(!has_esc(&out.stdout));
}

#[test]
fn theme_none_beats_color_always() {
    let out = run(markterm().args(["--theme", "none", "--color", "always", SAMPLE]));
    assert!(out.status.success());
    assert!(!has_esc(&out.stdout));
}

#[test]
fn no_color_set_disables_color() {
    let out = run(markterm()
        .env("NO_COLOR", "1")
        .args(["--color", "always", SAMPLE]));
    assert!(out.status.success());
    assert!(!has_esc(&out.stdout));
}

#[test]
fn empty_no_color_is_ignored() {
    let out = run(markterm()
        .env("NO_COLOR", "")
        .args(["--color", "always", SAMPLE]));
    assert!(out.status.success());
    assert!(has_esc(&out.stdout));
}

#[test]
fn invalid_theme_is_rejected() {
    let out = run(markterm().args(["--theme", "bogus", SAMPLE]));
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("bogus"));
}

#[test]
fn invalid_utf8_is_rendered_lossily() {
    let path = tmp_file("invalid-utf8.md", b"# Title\n\nbad \xff\xfe bytes here\n");
    let out = run(markterm().arg(&path));
    assert!(out.status.success(), "{:?}", out);
    let text = String::from_utf8(out.stdout).expect("output must be UTF-8");
    assert!(text.contains("Title"));
    assert!(text.contains('\u{FFFD}'));
    assert!(text.contains("bytes here"));
}

#[test]
fn stdin_with_dash() {
    let out = run_with_stdin(markterm().arg("-"), b"# From Stdin\n\nbody\n");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("From Stdin"));
}

#[test]
fn stdin_without_file_argument() {
    let out = run_with_stdin(&mut markterm(), b"# Implicit Stdin\n");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Implicit Stdin"));
}

#[test]
fn missing_file_reports_clean_error() {
    let out = run(markterm().arg("definitely-not-here.md"));
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        err.trim_end(),
        "markterm: definitely-not-here.md: No such file or directory"
    );
}

#[test]
fn no_pager_flag_dumps_output() {
    let out = run(markterm().args(["--no-pager", SAMPLE]));
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("markterm Sample Document"));
}

#[test]
fn broken_pipe_exits_cleanly() {
    let mut doc = String::new();
    for i in 0..20_000 {
        doc.push_str(&format!("Paragraph number {i} with some filler text.\n\n"));
    }
    let path = tmp_file("big.md", doc.as_bytes());

    let mut child = markterm()
        .args(["--color", "always"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut buf = [0u8; 256];
    stdout.read_exact(&mut buf).unwrap();
    drop(stdout);

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let status = child.wait().unwrap();
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
}
