//! Read a qmake `.pro` file, so checking a real project does not mean typing
//! `--package Qt6Quick --package Qt6Qml --include src ...` by hand.
//!
//! This is a deliberately small subset of qmake: `QT`, `SOURCES`, `HEADERS`,
//! `INCLUDEPATH` and `CONFIG += c++NN`. Anything else in the file is ignored,
//! and the tool says so rather than pretending to be qmake.

use std::path::{Path, PathBuf};

/// What a `.pro` file says we need to check it.
#[derive(Debug, Clone)]
pub struct ProjectSpec {
    pub name: String,
    /// `pkg-config` package names, e.g. `Qt6Quick`.
    pub packages: Vec<String>,
    /// `-std=` value derived from `CONFIG += c++NN`.
    pub std_version: String,
    /// Absolute include directories.
    pub include_dirs: Vec<PathBuf>,
    /// Absolute source and header paths.
    pub files: Vec<PathBuf>,
}

/// qmake module name → `pkg-config` package suffix.
fn package_for(module: &str) -> Option<String> {
    let suffix = match module {
        // pkg-config is not a simple CamelCase of the qmake name for these.
        "testlib" => "Test",
        "dbus" => "DBus",
        "printsupport" => "PrintSupport",
        "opengl" => "OpenGL",
        "openglwidgets" => "OpenGLWidgets",
        "uitools" => "UiTools",
        "statemachine" => "StateMachine",
        "quickcontrols2" => "QuickControls2",
        "quickwidgets" => "QuickWidgets",
        "svgwidgets" => "SvgWidgets",
        "texttospeech" => "TextToSpeech",
        "core" | "gui" | "widgets" | "qml" | "quick" | "network" | "sql"
        | "xml" | "svg" | "concurrent" | "websockets" | "serialport" | "positioning"
        | "multimedia" | "charts" | "pdf" => {
            // Camel-case the first letter: `quick` -> `Quick`.
            let mut chars = module.chars();
            let first = chars.next()?.to_ascii_uppercase();
            return Some(format!("Qt6{first}{}", chars.as_str()));
        }
        _ => return None,
    };
    Some(format!("Qt6{suffix}"))
}

/// Join `\`-continued lines into one statement per logical line.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim_end();
        let continued = line.ends_with('\\');
        let piece = line.trim_end_matches('\\').trim();
        if !current.is_empty() && !piece.is_empty() {
            current.push(' ');
        }
        current.push_str(piece);
        if !continued {
            if !current.trim().is_empty() {
                out.push(current.trim().to_string());
            }
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Directory holding the project file, always absolute.
///
/// `Path::parent()` of a bare `CMakeLists.txt` is the empty path, which
/// canonicalizes to nothing and silently turns every `-I` into a no-op. Treat
/// that as the current directory.
fn absolute_root(project_file: &Path) -> PathBuf {
    let parent = match project_file.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::canonicalize(&parent).unwrap_or(parent)
}

/// Parse a `.pro` file into the pieces this tool can use.
pub fn parse(pro: &Path) -> Result<ProjectSpec, String> {
    let text = std::fs::read_to_string(pro)
        .map_err(|e| format!("cannot read {}: {e}", pro.display()))?;
    // Absolute on purpose: the sources are copied to a temporary directory
    // before compiling, so a relative include directory would point nowhere.
    let root = absolute_root(pro);

    let mut modules: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    let mut headers: Vec<String> = Vec::new();
    let mut include_paths: Vec<PathBuf> = Vec::new();
    let mut std_version = "c++17".to_string();
    let mut name = pro
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    for line in logical_lines(&text) {
        let Some((key, value)) = line.split_once(operator_split(&line)) else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key {
            "QT" => {
                for module in value.split_whitespace() {
                    if let Some(package) = package_for(module) {
                        if !modules.iter().any(|seen| seen == &package) {
                            modules.push(package);
                        }
                    }
                }
            }
            "SOURCES" => sources.extend(value.split_whitespace().map(str::to_string)),
            "HEADERS" => headers.extend(value.split_whitespace().map(str::to_string)),
            "INCLUDEPATH" => include_paths.extend(value.split_whitespace().map(PathBuf::from)),
            "TARGET" => name = value.to_string(),
            "CONFIG" => {
                for flag in value.split_whitespace() {
                    if flag.starts_with("c++") {
                        std_version = flag.to_string();
                    }
                }
            }
            _ => {}
        }
    }

    if sources.is_empty() && headers.is_empty() {
        return Err(format!(
            "{}: no SOURCES or HEADERS found — is this a qmake project?",
            pro.display()
        ));
    }

    // qmake resolves paths against the .pro directory; do the same.
    let files: Vec<PathBuf> = sources
        .iter()
        .chain(headers.iter())
        .map(|entry| {
            let candidate = root.join(entry);
            candidate
        })
        .filter(|path| path.exists())
        .collect();

    let mut include_dirs: Vec<PathBuf> = vec![root.clone()];
    for extra in include_paths {
        include_dirs.push(root.join(extra));
    }
    // Generated headers included as "backend.h" from a sibling source are the
    // common case in hand-written qmake projects: add each file's own folder.
    for file in &files {
        if let Some(dir) = file.parent() {
            if !include_dirs.iter().any(|seen| seen == dir) {
                include_dirs.push(dir.to_path_buf());
            }
        }
    }

    Ok(ProjectSpec {
        name,
        packages: if modules.is_empty() { vec!["Qt6Core".to_string()] } else { modules },
        std_version,
        include_dirs,
        files,
    })
}

/// The first assignment operator in a statement: `+=`, `-=`, `*=`, `=`.
fn operator_split(line: &str) -> &'static str {
    for operator in ["+=", "-=", "*=", "="] {
        if line.contains(operator) {
            return operator;
        }
    }
    "+="
}

/// Parse either a qmake `.pro` file or a `CMakeLists.txt`.
pub fn parse_any(path: &Path) -> Result<ProjectSpec, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("CMakeLists.txt") || name.ends_with(".cmake") {
        parse_cmake(path)
    } else if name.ends_with(".pro") || name.ends_with(".pri") {
        parse(path)
    } else {
        Err(format!(
            "{}: expected a .pro file or a CMakeLists.txt",
            path.display()
        ))
    }
}

/// Split CMake source into `(command, arguments)` pairs.
///
/// CMake commands span lines until the closing parenthesis, may contain quoted
/// arguments with spaces, and nest parentheses in conditions. Rather than guess
/// per line, walk the text and collect balanced calls.
fn commands(text: &str) -> Vec<(String, Vec<String>)> {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        // Skip to the next identifier that is followed by '('.
        if !bytes[i].is_ascii_alphabetic() && bytes[i] != '_' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_') {
            i += 1;
        }
        let name: String = bytes[start..i].iter().collect();
        while i < bytes.len() && bytes[i].is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != '(' {
            continue;
        }
        i += 1; // consume '('

        let mut depth = 1usize;
        let mut quoted = false;
        let mut argument = String::new();
        let mut args: Vec<String> = Vec::new();
        while i < bytes.len() {
            let c = bytes[i];
            i += 1;
            match c {
                '"' => {
                    quoted = !quoted;
                    argument.push(c);
                }
                '(' if !quoted => {
                    depth += 1;
                    argument.push(c);
                }
                ')' if !quoted => {
                    depth -= 1;
                    if depth == 0 {
                        if !argument.trim().is_empty() {
                            args.push(argument.trim().to_string());
                        }
                        break;
                    }
                    argument.push(c);
                }
                '#' if !quoted => {
                    // Comment runs to the end of the line.
                    while i < bytes.len() && bytes[i] != '\n' {
                        i += 1;
                    }
                }
                c if c.is_whitespace() && !quoted => {
                    if !argument.trim().is_empty() {
                        args.push(argument.trim().to_string());
                    }
                    argument.clear();
                }
                c => argument.push(c),
            }
        }

        out.push((name.to_ascii_lowercase(), args));
    }
    out
}

/// Replace `${NAME}` with the value recorded by `set(NAME ...)`.
fn expand(value: &str, vars: &std::collections::HashMap<String, String>) -> String {
    let mut current = value.to_string();
    for _ in 0..8 {
        let mut next = String::new();
        let mut rest = current.as_str();
        let mut replaced = false;
        while let Some(open) = rest.find("${") {
            next.push_str(&rest[..open]);
            match rest[open..].find('}') {
                Some(close) => {
                    let key = &rest[open + 2..open + close];
                    match vars.get(key) {
                        Some(found) => {
                            next.push_str(found);
                            replaced = true;
                        }
                        None => next.push_str(&rest[open..open + close + 1]),
                    }
                    rest = &rest[open + close + 1..];
                }
                None => {
                    next.push_str(&rest[open..]);
                    rest = "";
                }
            }
        }
        next.push_str(rest);
        current = next;
        if !replaced {
            break;
        }
    }
    current
}

fn is_cxx_file(name: &str) -> bool {
    [".cpp", ".cc", ".cxx", ".c", ".h", ".hpp", ".hxx"]
        .iter()
        .any(|ext| name.to_ascii_lowercase().ends_with(ext))
}

/// Parse the subset of CMakeLists.txt this tool can act on.
pub fn parse_cmake(path: &Path) -> Result<ProjectSpec, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let root = absolute_root(path);

    // CMake's own variables: a project that says
    // `target_include_directories(app PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})`
    // means the project root, and that is exactly what we need.
    let mut vars: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for builtin in [
        "CMAKE_CURRENT_SOURCE_DIR",
        "CMAKE_CURRENT_LIST_DIR",
        "CMAKE_SOURCE_DIR",
        "PROJECT_SOURCE_DIR",
        "CMAKE_CURRENT_BINARY_DIR",
    ] {
        vars.insert(builtin.to_string(), root.display().to_string());
    }
    let mut components: Vec<String> = Vec::new();
    let mut source_entries: Vec<String> = Vec::new();
    let mut include_entries: Vec<String> = Vec::new();
    let mut std_version = "c++17".to_string();
    let mut name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());
    let mut mentions_qt6 = false;

    for (command, raw_args) in commands(&text) {
        // A CMake list is space- or semicolon-separated, and a variable holding
        // one expands into several arguments: split after expansion, not before.
        let args: Vec<String> = raw_args
            .iter()
            .flat_map(|arg| {
                expand(arg, &vars)
                    .split([' ', ';'])
                    .filter(|piece| !piece.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
        if command.contains("qt6") || command.starts_with("qt_") {
            mentions_qt6 = true;
        }
        match command.as_str() {
            "set" => {
                if args.len() >= 2 {
                    if args[0].eq_ignore_ascii_case("CMAKE_CXX_STANDARD") {
                        std_version = format!("c++{}", args[1]);
                    }
                    vars.insert(args[0].clone(), args[1..].join(" "));
                }
            }
            "project" => {
                if let Some(first) = args.first() {
                    name = first.clone();
                }
            }
            "find_package" => {
                if args.first().map(|p| p.starts_with("Qt6")).unwrap_or(false) {
                    mentions_qt6 = true;
                    for (index, arg) in args.iter().enumerate() {
                        if arg.eq_ignore_ascii_case("COMPONENTS") {
                            components.extend(
                                args[index + 1..]
                                    .iter()
                                    .filter(|a| !a.eq_ignore_ascii_case("REQUIRED"))
                                    .cloned(),
                            );
                        }
                    }
                }
            }
            "add_executable" | "add_library" | "qt_add_executable" | "qt_add_qml_module"
            | "target_sources" | "qt_add_resources" => {
                for arg in &args {
                    if is_cxx_file(arg) {
                        source_entries.push(arg.clone());
                    }
                }
            }
            "target_link_libraries" => {
                for arg in &args {
                    if let Some(rest) = arg.strip_prefix("Qt6::") {
                        mentions_qt6 = true;
                        components.push(rest.to_string());
                    }
                }
            }
            "target_include_directories" | "include_directories" => {
                for arg in &args {
                    let keyword = matches!(
                        arg.as_str(),
                        "PRIVATE" | "PUBLIC" | "INTERFACE" | "SYSTEM" | "BEFORE" | "AFTER"
                    );
                    if !keyword && !arg.starts_with("$") {
                        include_entries.push(arg.clone());
                    }
                }
            }
            _ => {}
        }
    }

    // Deduplicate while keeping the order Qt declares.
    let mut packages: Vec<String> = Vec::new();
    for component in &components {
        let package = format!("Qt6{component}");
        if !packages.iter().any(|seen| seen == &package) {
            packages.push(package);
        }
    }
    if packages.is_empty() {
        if !mentions_qt6 {
            return Err(format!(
                "{}: no Qt6 component found — is this a Qt project?",
                path.display()
            ));
        }
        packages.push("Qt6Core".to_string());
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for entry in &source_entries {
        let candidate = root.join(entry);
        if candidate.exists() && !files.iter().any(|seen| seen == &candidate) {
            files.push(candidate);
        }
    }
    if files.is_empty() {
        return Err(format!(
            "{}: no C++ sources found in add_executable/target_sources",
            path.display()
        ));
    }

    let mut include_dirs: Vec<PathBuf> = vec![root.clone()];
    for entry in &include_entries {
        let candidate = root.join(entry);
        if candidate.exists() && !include_dirs.iter().any(|seen| seen == &candidate) {
            include_dirs.push(candidate);
        }
    }
    for file in &files {
        if let Some(dir) = file.parent() {
            if !include_dirs.iter().any(|seen| seen == dir) {
                include_dirs.push(dir.to_path_buf());
            }
        }
    }

    Ok(ProjectSpec { name, packages, std_version, include_dirs, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pro(body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qtcheck_pro_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("a.cpp"), "int main() { return 0; }\n").unwrap();
        std::fs::write(dir.join("b.h"), "#pragma once\n").unwrap();
        std::fs::write(dir.join("app.pro"), body).unwrap();
        dir
    }

    #[test]
    fn parses_modules_sources_and_std() {
        let dir = write_pro(
            "QT += core gui qml quick quickcontrols2 dbus network\n\
             CONFIG += c++20 release\n\
             TARGET = demo\n\
             SOURCES += a.cpp\n\
             HEADERS += \\\n    b.h\n\
             INCLUDEPATH += src\n",
        );
        let spec = parse(&dir.join("app.pro")).unwrap();
        assert_eq!(spec.name, "demo");
        assert_eq!(spec.std_version, "c++20");
        assert_eq!(spec.packages.len(), 7);
        assert!(spec.packages.contains(&"Qt6QuickControls2".to_string()));
        assert!(spec.packages.contains(&"Qt6DBus".to_string()));
        assert_eq!(spec.files.len(), 2, "files: {:?}", spec.files);
        assert!(spec.include_dirs.iter().any(|d| d.ends_with("src")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn continues_backslash_lines() {
        let dir = write_pro("QT += core \\\n    quick\nSOURCES += a.cpp\n");
        let spec = parse(&dir.join("app.pro")).unwrap();
        assert_eq!(spec.packages, vec!["Qt6Core".to_string(), "Qt6Quick".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_a_file_that_is_not_a_project() {
        let dir = write_pro("this is not qmake\n");
        let error = parse(&dir.join("app.pro")).unwrap_err();
        assert!(error.contains("no SOURCES or HEADERS"), "error: {error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn maps_qmake_module_names() {
        assert_eq!(package_for("testlib"), Some("Qt6Test".to_string()));
        assert_eq!(package_for("dbus"), Some("Qt6DBus".to_string()));
        assert_eq!(package_for("printsupport"), Some("Qt6PrintSupport".to_string()));
        assert_eq!(package_for("quick"), Some("Qt6Quick".to_string()));
        assert_eq!(package_for("webengine"), None);
    }

    fn write_cmake(body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qtcheck_cmake_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.cpp"), "int main() { return 0; }\n").unwrap();
        std::fs::write(dir.join("src/backend.cpp"), "int backend() { return 1; }\n").unwrap();
        std::fs::write(dir.join("src/backend.h"), "#pragma once\n").unwrap();
        std::fs::write(dir.join("src/Main.qml"), "import QtQuick\nItem {}\n").unwrap();
        std::fs::write(dir.join("CMakeLists.txt"), body).unwrap();
        dir
    }

    #[test]
    fn parses_cmake_components_sources_and_variables() {
        let dir = write_cmake(
            "cmake_minimum_required(VERSION 3.21)\n\
             project(DemoApp VERSION 1.0 LANGUAGES CXX)\n\
             set(CMAKE_CXX_STANDARD 20)\n\
             find_package(Qt6 REQUIRED COMPONENTS Core Gui Quick QuickControls2 DBus)\n\
             set(APP_SOURCES src/main.cpp src/backend.cpp)\n\
             qt_add_executable(app ${APP_SOURCES})\n\
             target_sources(app PRIVATE src/backend.h)\n\
             target_include_directories(app PRIVATE src INTERFACE include)\n\
             target_link_libraries(app PRIVATE Qt6::Quick)\n",
        );
        let spec = parse_cmake(&dir.join("CMakeLists.txt")).unwrap();
        assert_eq!(spec.name, "DemoApp");
        assert_eq!(spec.std_version, "c++20");
        assert_eq!(spec.packages[0], "Qt6Core");
        assert!(spec.packages.contains(&"Qt6DBus".to_string()), "{:?}", spec.packages);
        assert!(spec.packages.contains(&"Qt6QuickControls2".to_string()));
        // QML is not C++ and must not be handed to the compiler.
        let names: Vec<String> = spec
            .files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["main.cpp", "backend.cpp", "backend.h"]);
        assert!(spec.include_dirs.iter().any(|d| d.ends_with("src")));
        // Regression: relative include dirs pointed nowhere once the source was
        // copied to a temp directory.
        assert!(
            spec.include_dirs.iter().all(|d| d.is_absolute()),
            "include dirs must be absolute: {:?}",
            spec.include_dirs
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_cmake_that_is_not_a_qt_project() {
        let dir = write_cmake("project(Plain)\nadd_executable(app src/main.cpp)\n");
        let error = parse_cmake(&dir.join("CMakeLists.txt")).unwrap_err();
        assert!(error.contains("no Qt6 component"), "error: {error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_bare_project_name_still_yields_an_absolute_root() {
        let root = absolute_root(Path::new("CMakeLists.txt"));
        assert!(root.is_absolute(), "root: {}", root.display());
    }

    #[test]
    fn dispatches_on_the_file_name() {
        let dir = write_cmake("project(X)\nfind_package(Qt6 COMPONENTS Core)\nadd_executable(x src/main.cpp)\n");
        assert!(parse_any(&dir.join("CMakeLists.txt")).is_ok());
        assert!(parse_any(&dir.join("app.pro")).is_err());
        assert!(parse_any(&dir.join("notes.txt")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
