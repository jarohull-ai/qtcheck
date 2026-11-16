//! A project that CMake itself accepts and builds, then checked by qtcheck.
//!
//! The point is not that qtcheck can compile a file — the other tests cover
//! that — but that its idea of "the project's files, includes and modules"
//! matches what a real build reads. If CMake and qtcheck disagree about which
//! sources exist, this test fails.
//!
//! Skipped when `cmake` is not installed; on CI it runs.

use std::path::{Path, PathBuf};
use std::process::Command;

const CMAKE_LISTS: &str = r#"cmake_minimum_required(VERSION 3.21)
project(qtcheck_fixture LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 17)
find_package(Qt6 REQUIRED COMPONENTS Widgets)
# Turns on AUTOMOC: without it the Q_OBJECT class has no vtable and the link
# fails, which is a property of a real Qt project rather than of this test.
qt_standard_project_setup()
set(APP_SOURCES main.cpp widget.cpp)
qt_add_executable(app ${APP_SOURCES} widget.h)
target_include_directories(app PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(app PRIVATE Qt6::Widgets)
"#;

const WIDGET_H: &str = r#"#pragma once
#include <QWidget>
#include <QString>

class Widget : public QWidget {
    Q_OBJECT
public:
    explicit Widget(QWidget *parent = nullptr);
    QString label() const;
};
"#;

const WIDGET_CPP: &str = r#"#include "widget.h"

Widget::Widget(QWidget *parent) : QWidget(parent) {}

QString Widget::label() const
{
    return QStringLiteral("hello");
}
"#;

const MAIN_CPP: &str = r#"#include <QApplication>
#include "widget.h"

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    Widget window;
    window.show();
    return app.exec();
}
"#;

fn cmake_available() -> bool {
    Command::new("cmake")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn write_fixture() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qtcheck_cmake_project_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("cannot create fixture dir");
    for (name, body) in [
        ("CMakeLists.txt", CMAKE_LISTS),
        ("widget.h", WIDGET_H),
        ("widget.cpp", WIDGET_CPP),
        ("main.cpp", MAIN_CPP),
    ] {
        std::fs::write(dir.join(name), body).expect("cannot write fixture file");
    }
    dir
}

fn run(args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(args[0])
        .args(&args[1..])
        .current_dir(cwd)
        .output()
        .expect("cannot run command");
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

#[test]
fn qtcheck_sees_the_same_files_that_cmake_builds() {
    let dir = write_fixture();

    // 1. The fixture is a real project: CMake configures and builds it. Without
    // cmake installed this half cannot run, and saying so is better than
    // silently claiming the project was validated.
    if cmake_available() {
        let (ok, log) = run(&["cmake", "-S", ".", "-B", "build"], &dir);
        assert!(ok, "cmake configure failed:\n{log}");
        let (ok, log) = run(&["cmake", "--build", "build"], &dir);
        assert!(ok, "cmake build failed:\n{log}");
    } else {
        eprintln!("cmake is not installed here: the build half of this test was skipped");
    }

    // 2. qtcheck reads the same project without building it.
    let spec = qtcheck::project::parse_any(&dir.join("CMakeLists.txt")).expect("cannot parse fixture");
    let mut names: Vec<String> = spec
        .files
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["main.cpp", "widget.cpp", "widget.h"]);
    assert_eq!(spec.packages, vec!["Qt6Widgets".to_string()]);
    assert_eq!(spec.std_version, "c++17");

    // 3. Every file in it passes, headers included (widget.h needs moc).
    let env = qtcheck::detect_env(&spec.packages).expect("Qt 6 development files missing");
    let opts = qtcheck::CheckOptions {
        std_version: spec.std_version.clone(),
        packages: spec.packages.clone(),
        include_dirs: spec.include_dirs.clone(),
        timeout_ms: 20_000,
    };
    for file in &spec.files {
        let code = std::fs::read_to_string(file).expect("cannot read fixture source");
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let report = qtcheck::check_with_env(&code, &name, &opts, &env);
        assert!(
            report.success,
            "{} failed:\n{}",
            file.display(),
            report.stderr
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
