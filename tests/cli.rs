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
fn project_checks_every_file_named_by_the_pro() {
    let dir = std::env::temp_dir().join(format!(
        "qtcheck_pro_cli_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(dir.join("src")).expect("cannot create project dir");
    std::fs::write(
        dir.join("src/good.cpp"),
        "#include <QString>\nQString ok() { return QStringLiteral(\"x\"); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/bad.cpp"),
        "#include <QString>\nint nope() { return QString(\"x\"); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("app.pro"),
        "QT += core\nCONFIG += c++17\nSOURCES += src/good.cpp src/bad.cpp\n",
    )
    .unwrap();

    let pro = dir.join("app.pro").to_string_lossy().to_string();
    let (code, stdout, stderr) = run(&["project", &pro], "");
    assert_eq!(code, 1, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("1/2 files clean"), "stdout: {stdout}");

    let (code, stdout, _) = run(&["project", &pro, "--json"], "");
    assert_eq!(code, 1);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("report is not JSON");
    assert_eq!(report["total"], serde_json::Value::from(2));
    assert_eq!(report["failed"], serde_json::Value::from(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn project_jobs_and_github_output_keep_the_same_verdict() {
    let dir = std::env::temp_dir().join(format!(
        "qtcheck_jobs_cli_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(dir.join("src")).expect("cannot create project dir");
    std::fs::write(
        dir.join("src/good.cpp"),
        "#include <QString>\nQString ok() { return QStringLiteral(\"x\"); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/bad.cpp"),
        "#include <QString>\nint nope() { return QString(\"x\"); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("app.pro"),
        "QT += core\nCONFIG += c++17\nSOURCES += src/good.cpp src/bad.cpp\n",
    )
    .unwrap();
    let pro = dir.join("app.pro").to_string_lossy().to_string();

    // One worker must reach the same verdict as all of them.
    let (code, stdout, stderr) = run(&["project", &pro, "--jobs", "1"], "");
    assert_eq!(code, 1, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("1/2 files clean"), "stdout: {stdout}");

    let (code, stdout, stderr) = run(&["project", &pro, "--jobs", "4"], "");
    assert_eq!(code, 1, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("1/2 files clean"), "stdout: {stdout}");

    // GitHub annotations: one machine-readable line per problem, pointing at
    // the file and line, and a job-level failure for the summarised exit.
    let (code, stdout, _) = run(&["project", &pro, "--github", "--jobs", "1"], "");
    assert_eq!(code, 1);
    assert!(
        stdout.contains("::error file=") && stdout.contains(",line="),
        "no annotation with a location: {stdout}"
    );
    assert!(
        stdout.contains("bad.cpp"),
        "the annotation does not name the failing file: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_project_that_names_no_files_is_not_reported_as_clean() {
    let dir = std::env::temp_dir().join(format!(
        "qtcheck_empty_cli_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("cannot create dir");
    // A CMake project whose sources come from a function call: the parser
    // cannot see them, and saying "clean" would be a lie.
    std::fs::write(
        dir.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.16)\nproject(app)\nfind_package(Qt6 COMPONENTS Core)\nmy_collect_sources(SOURCES)\n",
    )
    .unwrap();
    std::fs::write(dir.join("empty.pro"), "QT += core\n").unwrap();

    for name in ["CMakeLists.txt", "empty.pro"] {
        let pro = dir.join(name).to_string_lossy().to_string();
        let (code, stdout, stderr) = run(&["project", &pro], "");
        assert_eq!(code, 4, "{name}: stdout: {stdout}\nstderr: {stderr}");
        assert!(!stdout.contains("clean"), "{name}: false green: {stdout}");
        assert!(
            stderr.contains("qtcheck:") && stderr.contains("no "),
            "{name}: stderr does not explain itself: {stderr}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn check_accepts_several_files_at_once() {
    let dir = std::env::temp_dir().join(format!(
        "qtcheck_many_cli_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("cannot create dir");
    let good = dir.join("good.cpp");
    let bad = dir.join("bad.cpp");
    std::fs::write(&good, GOOD).unwrap();
    std::fs::write(&bad, BAD_TYPE).unwrap();
    let (good, bad) = (
        good.to_string_lossy().to_string(),
        bad.to_string_lossy().to_string(),
    );

    let (code, stdout, stderr) = run(&["check", &good, &bad], "");
    assert_eq!(code, 1, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("1/2 files clean"), "stdout: {stdout}");
    assert!(stdout.contains("bad.cpp"), "stdout: {stdout}");

    let (code, stdout, _) = run(&["check", &good, &bad, "--json"], "");
    assert_eq!(code, 1);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("report is not JSON");
    assert_eq!(report["total"], serde_json::Value::from(2));
    assert_eq!(report["failed"], serde_json::Value::from(1));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn port_rewrites_a_whole_project_and_verifies_it() {
    let dir = std::env::temp_dir().join(format!(
        "qtcheck_port_proj_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(dir.join("src")).expect("cannot create project dir");
    std::fs::write(dir.join("src/legacy.cpp"), QT5_LEFTOVER).unwrap();
    std::fs::write(
        dir.join("app.pro"),
        "QT += core\nCONFIG += c++17\nSOURCES += src/legacy.cpp\n",
    )
    .unwrap();
    let pro = dir.join("app.pro").to_string_lossy().to_string();

    let (code, stdout, stderr) = run(&["port", "--project", &pro, "--write"], "");
    let rewritten = std::fs::read_to_string(dir.join("src/legacy.cpp")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(rewritten.contains("Qt::SkipEmptyParts"), "file: {rewritten}");
    assert!(
        stdout.contains("compile"),
        "the project rewrite was not verified with the compiler: {stdout}"
    );
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
