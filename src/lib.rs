//! `qtcheck` — is this Qt 6 code real, or does it only look like Qt?
//!
//! The loop it exists for: an assistant writes Qt code → `qtcheck` says which
//! symbols do not exist, which Qt 5 spellings Qt 6 removed, and what to write
//! instead → the code gets fixed before anyone waits on a full project build.
//!
//! It compiles one translation unit with the real headers of the Qt 6 that is
//! installed (`pkg-config --cflags`), runs `moc` when the source needs a
//! meta-object, and turns the compiler's text output into structured
//! diagnostics. No project, no build system, no link step: a check costs a few
//! hundred milliseconds.

use serde::Serialize;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub mod port;
pub mod project;

/// Default wall-clock budget for one check.
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// The Qt modules checked when the caller does not say otherwise.
pub const DEFAULT_PACKAGES: [&str; 3] = ["Qt6Core", "Qt6Gui", "Qt6Widgets"];

/// Knobs for one check.
#[derive(Debug, Clone)]
pub struct CheckOptions {
    /// `-std=` value, e.g. `c++17`.
    pub std_version: String,
    /// `pkg-config` packages whose include flags are used.
    pub packages: Vec<String>,
    /// Extra include directories, e.g. a project's `include/`.
    pub include_dirs: Vec<PathBuf>,
    /// Wall-clock budget for compiler and `moc` together.
    pub timeout_ms: u64,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            std_version: "c++17".to_string(),
            packages: DEFAULT_PACKAGES.iter().map(|p| p.to_string()).collect(),
            include_dirs: Vec::new(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

/// The Qt installation this check will use.
#[derive(Debug, Clone, Serialize)]
pub struct QtEnv {
    /// Version reported by `pkg-config --modversion Qt6Core`.
    pub qt_version: String,
    /// Absolute path of `moc`.
    pub moc: PathBuf,
    /// C++ compiler that will be invoked.
    pub compiler: String,
    /// Include flags, deduplicated and in a stable order.
    pub cflags: Vec<String>,
}

/// One compiler message.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// `error`, `warning` or `fatal`.
    pub severity: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

/// Outcome of one check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub success: bool,
    pub duration_ms: f64,
    pub qt_version: String,
    /// True when `moc` had to run for this source.
    pub moc_ran: bool,
    pub timed_out: bool,
    /// Tooling problems only (`E_NO_QT6`, `E_NO_COMPILER`, `E_MOC`, `E_TIMEOUT`).
    /// Ordinary compile errors are reported as diagnostics, not codes.
    pub error_codes: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub stderr: String,
}

impl CheckReport {
    /// Short label: `pass`, `fail`, `timeout`.
    pub fn status(&self) -> &'static str {
        if self.timed_out {
            "timeout"
        } else if self.success {
            "pass"
        } else {
            "fail"
        }
    }

    /// Exit code for the CLI: 0 pass, 1 the code is wrong, 3 timeout,
    /// 4 the tool could not do its job, 64 bad command line.
    pub fn cli_exit_code(&self) -> u8 {
        if self.timed_out {
            3
        } else if !self.error_codes.is_empty() {
            4
        } else if self.success {
            0
        } else {
            1
        }
    }
}

/// Locate `moc` for the Qt 6 installation on this machine.
fn locate_moc() -> Option<PathBuf> {
    if let Ok(out) = Command::new("qmake6")
        .args(["-query", "QT_HOST_LIBEXECS"])
        .output()
    {
        if out.status.success() {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !dir.is_empty() {
                for name in ["moc", "moc-qt6"] {
                    let candidate = PathBuf::from(&dir).join(name);
                    if candidate.exists() {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    [
        "/usr/lib/qt6/moc",
        "/usr/lib/qt6/libexec/moc",
        "/usr/libexec/moc",
        "/usr/bin/moc",
        "/usr/local/lib/qt6/moc",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.exists())
}

/// Read the Qt environment, or say precisely what is missing.
pub fn detect_env(packages: &[String]) -> Result<QtEnv, String> {
    if packages.is_empty() {
        return Err("no Qt packages requested".to_string());
    }

    let qt_version = Command::new("pkg-config")
        .args(["--modversion", "Qt6Core"])
        .output()
        .map_err(|e| format!("cannot run pkg-config: {e}"))?
        .pipe(|out| -> Result<String, String> {
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                Err("Qt 6 development files not found (pkg-config Qt6Core failed). \
                     Install qt6-base-dev, or set PKG_CONFIG_PATH."
                    .to_string())
            }
        })?;

    let mut cflags: Vec<String> = Vec::new();
    for package in packages {
        let out = Command::new("pkg-config")
            .args(["--cflags", package])
            .output()
            .map_err(|e| format!("cannot run pkg-config: {e}"))?;
        if !out.status.success() {
            // A missing optional module is only fatal when the caller asked
            // for it by name and Qt itself is fine.
            continue;
        }
        for flag in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            if !cflags.iter().any(|seen| seen == flag) {
                cflags.push(flag.to_string());
            }
        }
    }
    if cflags.is_empty() {
        return Err(format!(
            "pkg-config knows Qt6Core but returned no include flags for {packages:?}"
        ));
    }

    let moc = locate_moc().ok_or_else(|| {
        "moc not found (looked in qmake6 -query QT_HOST_LIBEXECS and the usual Qt 6 paths). \
         Install qt6-base-dev."
            .to_string()
    })?;

    let compiler = std::env::var("CXX").unwrap_or_else(|_| "c++".to_string());
    Command::new(&compiler)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("C++ compiler {compiler:?} is not usable: {e}"))?;

    Ok(QtEnv { qt_version, moc, compiler, cflags })
}

/// Helper so `?` can be used on process output without a macro dance.
trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl Pipe for std::process::Output {}

/// Does this source declare a meta-object that `moc` has to process?
pub fn needs_moc(code: &str) -> bool {
    ["Q_OBJECT", "Q_GADGET", "Q_NAMESPACE", "Q_NAMESPACE_EXPORT"]
        .iter()
        .any(|marker| code.contains(marker))
}

/// Check `code`, detecting the Qt installation first.
pub fn check(code: &str, opts: &CheckOptions) -> CheckReport {
    match detect_env(&opts.packages) {
        Ok(env) => check_with_env(code, "snippet.cpp", opts, &env),
        Err(message) => tooling_failure(message, "E_NO_QT6"),
    }
}

/// Check `code` against an already detected environment.
///
/// `file_name` matters for sources that end with `#include "foo.moc"`: the
/// generated meta-object is written next to the source under that name.
pub fn check_with_env(
    code: &str,
    file_name: &str,
    opts: &CheckOptions,
    env: &QtEnv,
) -> CheckReport {
    let start = Instant::now();
    let dir = temp_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return tooling_failure(format!("cannot create {}: {e}", dir.display()), "E_IO");
    }
    let source = dir.join(file_name);
    if let Err(e) = std::fs::write(&source, code) {
        let _ = std::fs::remove_dir_all(&dir);
        return tooling_failure(format!("cannot write {}: {e}", source.display()), "E_IO");
    }

    let mut include_dirs: Vec<PathBuf> = vec![dir.clone()];
    include_dirs.extend(opts.include_dirs.iter().cloned());

    let mut moc_ran = false;
    if needs_moc(code) {
        moc_ran = true;
        let moc_out = dir.join(format!(
            "{}.moc",
            source.file_stem().unwrap_or_default().to_string_lossy()
        ));
        match Command::new(&env.moc)
            .arg("-o")
            .arg(&moc_out)
            .arg(&source)
            // Diagnostics are parsed and read by agents: keep them in one
            // language no matter what locale the caller has.
            .env("LC_ALL", "C")
            .output()
        {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let _ = std::fs::remove_dir_all(&dir);
                // moc refusing a source is a source problem, not tooling.
                let mut report = tooling_failure(String::new(), "E_MOC");
                report.error_codes.clear();
                report.diagnostics = vec![Diagnostic {
                    severity: "error".to_string(),
                    line: None,
                    column: None,
                    message: format!("moc failed: {}", first_line(&stderr)),
                }];
                report.stderr = stderr;
                report.duration_ms = start.elapsed().as_secs_f64() * 1000.0;
                report.qt_version = env.qt_version.clone();
                report.moc_ran = moc_ran;
                return report;
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return tooling_failure(format!("cannot run moc: {e}"), "E_MOC");
            }
        }
    }

    let mut cmd = Command::new(&env.compiler);
    cmd.arg(format!("-std={}", opts.std_version))
        .arg("-fsyntax-only")
        .arg("-fPIC");
    for flag in &env.cflags {
        cmd.arg(flag);
    }
    for inc in &include_dirs {
        cmd.arg("-I").arg(inc);
    }
    cmd.env("LC_ALL", "C")
        .arg(&source)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return tooling_failure(format!("cannot start {}: {e}", env.compiler), "E_NO_COMPILER");
        }
    };

    let err_reader = child.stderr.take().map(spawn_reader);
    let out_reader = child.stdout.take().map(spawn_reader);

    let deadline = Duration::from_millis(opts.timeout_ms.max(200));
    let mut timed_out = false;
    let mut status_code: Option<i32> = None;
    while start.elapsed() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                status_code = status.code();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break,
        }
    }
    if status_code.is_none() && !child.try_wait().map(|s| s.is_some()).unwrap_or(false) {
        let _ = child.kill();
        let _ = child.wait();
        timed_out = true;
    }

    let stderr = String::from_utf8_lossy(
        &err_reader.and_then(|h| h.join().ok()).unwrap_or_default(),
    )
    .to_string();
    let stdout = String::from_utf8_lossy(
        &out_reader.and_then(|h| h.join().ok()).unwrap_or_default(),
    )
    .to_string();
    let _ = std::fs::remove_dir_all(&dir);

    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
    if timed_out {
        let mut report = tooling_failure(
            format!("check exceeded {} ms and was killed", opts.timeout_ms),
            "E_TIMEOUT",
        );
        report.timed_out = true;
        report.duration_ms = duration_ms;
        report.qt_version = env.qt_version.clone();
        report.moc_ran = moc_ran;
        report.stderr = stderr;
        return report;
    }

    let text = if stdout.trim().is_empty() { stderr.clone() } else { format!("{stderr}\n{stdout}") };
    CheckReport {
        success: status_code == Some(0),
        duration_ms,
        qt_version: env.qt_version.clone(),
        moc_ran,
        timed_out: false,
        error_codes: Vec::new(),
        diagnostics: parse_diagnostics(&text),
        stderr: text,
    }
}

fn temp_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("qtcheck_{}_{}", std::process::id(), nanos))
}

fn spawn_reader<T: Read + Send + 'static>(mut pipe: T) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn tooling_failure(message: String, code: &str) -> CheckReport {
    CheckReport {
        success: false,
        duration_ms: 0.0,
        qt_version: String::new(),
        moc_ran: false,
        timed_out: false,
        error_codes: vec![code.to_string()],
        diagnostics: if message.is_empty() {
            Vec::new()
        } else {
            vec![Diagnostic {
                severity: "error".to_string(),
                line: None,
                column: None,
                message,
            }]
        },
        stderr: String::new(),
    }
}

/// Turn `g++`'s human output into `path:line:col: error: message` records.
///
/// Notes and the caret art are dropped: a caller that shows them can take
/// `stderr` verbatim, and a caller that parses wants one record per problem.
fn parse_diagnostics(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((location, rest)) = line.split_once(": ") else {
            continue;
        };
        let severity = if rest.starts_with("fatal error:") {
            "fatal"
        } else if rest.starts_with("error:") {
            "error"
        } else if rest.starts_with("warning:") {
            "warning"
        } else {
            continue;
        };
        let message = rest
            .splitn(2, ':')
            .nth(1)
            .map(|m| m.trim().to_string())
            .unwrap_or_else(|| rest.to_string());

        // `location` is `file:line:col` for diagnostics; anything else (the
        // "In file included from" prelude) is context we already keep.
        let mut parts = location.rsplitn(3, ':');
        let column = parts.next().and_then(|c| c.trim().parse::<u32>().ok());
        let line_no = parts.next().and_then(|l| l.trim().parse::<u32>().ok());
        if line_no.is_none() {
            continue;
        }

        out.push(Diagnostic {
            severity: severity.to_string(),
            line: line_no,
            column,
            message,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_meta_object_need() {
        assert!(needs_moc("class A : public QObject { Q_OBJECT };"));
        assert!(needs_moc("#include <QObject>\nQ_GADGET enum class E { A };"));
        assert!(!needs_moc("int main() { return 0; }"));
    }

    #[test]
    fn parses_compiler_lines_into_diagnostics() {
        let text = "In file included from snippet.cpp:1:\n\
                    /usr/include/qt6/QtCore/qstring.h:245:10: note: declared here\n\
                    snippet.cpp:12:5: error: 'SkipEmptyParts' is not a member of 'QString'\n\
                    snippet.cpp:20:1: warning: unused variable 'x' [-Wunused-variable]\n\
                     ^~~~\n";
        let diags = parse_diagnostics(text);
        assert_eq!(diags.len(), 2, "diags: {diags:?}");
        assert_eq!(diags[0].severity, "error");
        assert_eq!(diags[0].line, Some(12));
        assert_eq!(diags[0].column, Some(5));
        assert!(diags[0].message.contains("SkipEmptyParts"));
        assert_eq!(diags[1].severity, "warning");
    }

    #[test]
    fn tooling_failure_exits_with_four() {
        let report = tooling_failure("no Qt".to_string(), "E_NO_QT6");
        assert_eq!(report.cli_exit_code(), 4);
    }

    #[test]
    fn valid_code_passes_and_invalid_code_fails() {
        let env = match detect_env(&DEFAULT_PACKAGES.iter().map(|p| p.to_string()).collect::<Vec<_>>()) {
            Ok(env) => env,
            Err(e) => panic!("this test needs Qt 6 development files: {e}"),
        };
        let opts = CheckOptions::default();
        let good = check_with_env(
            "#include <QString>\nQString greet() { return QStringLiteral(\"hi\"); }\n",
            "snippet.cpp",
            &opts,
            &env,
        );
        assert!(good.success, "stderr: {}", good.stderr);
        assert_eq!(good.cli_exit_code(), 0);

        let bad = check_with_env(
            "#include <QString>\nint f() { return QString(\"x\"); }\n",
            "snippet.cpp",
            &opts,
            &env,
        );
        assert!(!bad.success);
        assert_eq!(bad.cli_exit_code(), 1);
        assert!(!bad.diagnostics.is_empty(), "no diagnostics parsed");
    }

    #[test]
    fn class_with_q_object_is_checked_with_moc() {
        let env = match detect_env(&DEFAULT_PACKAGES.iter().map(|p| p.to_string()).collect::<Vec<_>>()) {
            Ok(env) => env,
            Err(e) => panic!("this test needs Qt 6 development files: {e}"),
        };
        // Qt's own convention: the meta-object include goes last.
        let code = "#include <QObject>\nclass Widget : public QObject {\n  Q_OBJECT\npublic:\n  explicit Widget(QObject *parent = nullptr) : QObject(parent) {}\n};\n#include \"widget.moc\"\n";
        let report = check_with_env(code, "widget.cpp", &CheckOptions::default(), &env);
        assert!(report.moc_ran, "moc should have run");
        assert!(report.success, "stderr: {}", report.stderr);
    }
}
