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

/// Parse a `.pro` file into the pieces this tool can use.
pub fn parse(pro: &Path) -> Result<ProjectSpec, String> {
    let text = std::fs::read_to_string(pro)
        .map_err(|e| format!("cannot read {}: {e}", pro.display()))?;
    let root = pro
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

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
}
