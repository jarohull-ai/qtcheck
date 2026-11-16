//! End-to-end checks of the binary: what a script or an agent will actually see.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_qtcheck"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qtcheck");
    child
        .stdin
        .take()
        .expect("no stdin")
        .write_all(stdin.as_bytes())
        .expect("failed to write stdin");
    let out = child.wait_with_output().expect("failed to read output");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

const GOOD: &str = "#include <QString>\nQString greet() { return QStringLiteral(\"hi\"); }\n";
const BAD_TYPE: &str = "#include <QString>\nint count() { return QString(\"x\"); }\n";
const QT5_LEFTOVER: &str = "#include <QString>\n#include <QStringList>\nQStringList parts() { return QString(\"a,b\").split(',', QString::SkipEmptyParts); }\n";

#[test]
fn valid_code_exits_zero() {
    let (code, stdout, stderr) = run(&["check"], GOOD);
    assert_eq!(code, 0, "stdout: {stdout} stderr: {stderr}");
    assert!(stdout.contains("pass"), "stdout: {stdout}");
}

#[test]
fn broken_code_exits_one_and_points_at_the_line() {
    let (code, stdout, _) = run(&["check"], BAD_TYPE);
    assert_eq!(code, 1, "stdout: {stdout}");
    assert!(stdout.contains("error"), "stdout: {stdout}");
    assert!(stdout.contains("2:"), "expected the error on line 2: {stdout}");
}

#[test]
fn failing_check_hints_at_the_port_command() {
    let (code, stdout, _) = run(&["check"], QT5_LEFTOVER);
    assert_eq!(code, 1);
    assert!(stdout.contains("qtcheck port"), "stdout: {stdout}");
}

#[test]
fn usage_error_exits_sixty_four() {
    let (code, _, stderr) = run(&["check", "--not-a-flag"], "");
    assert_eq!(code, 64, "stderr: {stderr}");
}

#[test]
fn clean_source_has_nothing_to_port() {
    let (code, stdout, _) = run(&["port"], GOOD);
    assert_eq!(code, 0, "stdout: {stdout}");
    assert!(stdout.contains("no Qt 5 spellings"), "stdout: {stdout}");
}

#[test]
fn qt5_leftover_is_reported_with_its_replacement() {
    let (code, stdout, _) = run(&["port"], QT5_LEFTOVER);
    assert_eq!(code, 1, "stdout: {stdout}");
    assert!(stdout.contains("QString::SkipEmptyParts"), "stdout: {stdout}");
    assert!(stdout.contains("Qt::SkipEmptyParts"), "stdout: {stdout}");
    assert!(
        stdout.contains("3:"),
        "expected the leftover on line 3: {stdout}"
    );
}

#[test]
fn port_json_is_parseable() {
    let (code, stdout, _) = run(&["port", "--json"], QT5_LEFTOVER);
    assert_eq!(code, 1);
    let hits: serde_json::Value = serde_json::from_str(&stdout).expect("findings are not JSON");
    let array = hits.as_array().expect("findings are not an array");
    assert_eq!(array.len(), 1, "findings: {hits}");
    assert_eq!(array[0]["auto"], serde_json::Value::Bool(true));
    assert_eq!(array[0]["line"], serde_json::Value::from(3));
}

#[test]
fn port_write_rewrites_the_file_and_verifies_it() {
    let path = std::env::temp_dir().join(format!(
        "qtcheck_cli_{}_{}.cpp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, QT5_LEFTOVER).expect("cannot write temp source");
    let file = path.to_string_lossy().to_string();

    let (code, stdout, stderr) = run(&["port", "--write", &file], "");
    let rewritten = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);

    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(rewritten.contains("Qt::SkipEmptyParts"), "file: {rewritten}");
    assert!(
        stdout.contains("now compiles"),
        "the rewrite was not verified with the compiler: {stdout}"
    );
}
