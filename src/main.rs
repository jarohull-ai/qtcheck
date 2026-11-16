use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use qtcheck::{check_with_env, detect_env, port, CheckOptions, DEFAULT_PACKAGES};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "qtcheck",
    version,
    about = "Check Qt 6 code without building a project: real compiler diagnostics plus the Qt 5 spellings Qt 6 removed"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Type-check one Qt 6 source file (or stdin) against the installed Qt.
    Check {
        /// Path to a .cpp/.h file. Omit to read stdin.
        file: Option<PathBuf>,

        /// Machine-readable report on stdout.
        #[arg(long)]
        json: bool,

        /// C++ standard to compile with.
        #[arg(long, default_value = "c++17")]
        std: String,

        /// pkg-config package to take include flags from (repeatable).
        #[arg(long = "package", value_name = "PKG")]
        packages: Vec<String>,

        /// Extra include directory (repeatable).
        #[arg(long = "include", value_name = "DIR")]
        includes: Vec<PathBuf>,

        /// Wall-clock budget for moc and the compiler, in milliseconds.
        #[arg(long, default_value_t = 10_000)]
        timeout_ms: u64,
    },

    /// List Qt 5 spellings that Qt 6 removed, with the replacement for each.
    Port {
        /// Path to a .cpp/.h file. Omit to read stdin.
        file: Option<PathBuf>,

        /// Rewrite the automatic renames in place.
        #[arg(long)]
        write: bool,

        /// Machine-readable list of findings.
        #[arg(long)]
        json: bool,
    },

    /// Check every source and header named by a qmake .pro file.
    Project {
        /// Path to the .pro file.
        pro: PathBuf,

        /// Machine-readable report on stdout.
        #[arg(long)]
        json: bool,
    },

    /// Report which Qt installation this tool would use.
    Doctor,
}

fn read_code(file: &Option<PathBuf>) -> Result<String, String> {
    match file {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display())),
        None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .map_err(|e| format!("cannot read stdin: {e}"))?;
            Ok(buf)
        }
    }
}

fn options_from(
    std_version: String,
    packages: &[String],
    includes: &[PathBuf],
    timeout_ms: u64,
) -> CheckOptions {
    CheckOptions {
        std_version,
        packages: if packages.is_empty() {
            DEFAULT_PACKAGES.iter().map(|p| p.to_string()).collect()
        } else {
            packages.to_vec()
        },
        include_dirs: includes.to_vec(),
        timeout_ms,
    }
}

fn main() -> ExitCode {
    // Parse by hand: clap's default failure code (2) must not look like a
    // verdict from the compiler.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let kind = err.kind();
            let _ = err.print();
            return if matches!(kind, ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(64)
            };
        }
    };

    match cli.cmd {
        Cmd::Check { file, json, std, packages, includes, timeout_ms } => {
            let code = match read_code(&file) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("qtcheck: {e}");
                    return ExitCode::from(4);
                }
            };
            let opts = options_from(std, &packages, &includes, timeout_ms);
            let env = match detect_env(&opts.packages) {
                Ok(env) => env,
                Err(e) => {
                    eprintln!("qtcheck: {e}");
                    return ExitCode::from(4);
                }
            };
            let file_name = file
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "snippet.cpp".to_string());

            let report = check_with_env(&code, &file_name, &opts, &env);

            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("qtcheck: cannot serialize report: {e}");
                        return ExitCode::from(4);
                    }
                }
            } else if report.success {
                println!(
                    "✓ pass  ({} ms, Qt {}, moc: {})",
                    report.duration_ms.round(),
                    report.qt_version,
                    if report.moc_ran { "yes" } else { "no" }
                );
            } else {
                println!(
                    "✗ {}  ({} ms, Qt {})",
                    report.status(),
                    report.duration_ms.round(),
                    report.qt_version
                );
                for diagnostic in &report.diagnostics {
                    let location = match (diagnostic.line, diagnostic.column) {
                        (Some(line), Some(column)) => format!("{line}:{column}"),
                        (Some(line), None) => format!("{line}"),
                        _ => "-".to_string(),
                    };
                    println!("  {} {location}: {}", diagnostic.severity, diagnostic.message);
                }
                if report.diagnostics.is_empty() && !report.stderr.trim().is_empty() {
                    for line in report.stderr.lines().take(10) {
                        println!("  {line}");
                    }
                }
                // The reason this tool exists: name the Qt 5 leftovers.
                let hits = port::scan(&code);
                if !hits.is_empty() {
                    println!();
                    println!(
                        "  {} Qt 5 spelling(s) found — run `qtcheck port` for the Qt 6 replacements",
                        hits.len()
                    );
                }
            }

            ExitCode::from(report.cli_exit_code())
        }

        Cmd::Port { file, write, json } => {
            let code = match read_code(&file) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("qtcheck: {e}");
                    return ExitCode::from(4);
                }
            };
            let hits = port::scan(&code);

            if json {
                match serde_json::to_string_pretty(&hits) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("qtcheck: cannot serialize findings: {e}");
                        return ExitCode::from(4);
                    }
                }
            } else if hits.is_empty() {
                println!("no Qt 5 spellings found");
            } else {
                let name = file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<stdin>".to_string());
                println!("{name}: {} Qt 5 spelling(s)", hits.len());
                for hit in &hits {
                    println!("  {}", hit.describe());
                    println!("      {}", hit.note);
                }
                let auto = hits.iter().filter(|hit| hit.auto).count();
                println!();
                println!(
                    "{auto} can be rewritten automatically, {} need a human.",
                    hits.len() - auto
                );
                if auto > 0 && !write {
                    println!("Re-run with --write to apply the automatic ones.");
                }
            }

            if !write {
                return ExitCode::from(if hits.is_empty() { 0 } else { 1 });
            }

            let Some(path) = file.as_ref() else {
                eprintln!("qtcheck: --write needs a file; nothing was changed");
                return ExitCode::from(4);
            };
            let (rewritten, applied) = port::apply(&code, &hits);
            if applied == 0 {
                println!("nothing to rewrite automatically");
                return ExitCode::from(if hits.is_empty() { 0 } else { 1 });
            }
            if let Err(e) = std::fs::write(path, &rewritten) {
                eprintln!("qtcheck: cannot write {}: {e}", path.display());
                return ExitCode::from(4);
            }
            println!("rewrote {applied} occurrence(s) in {}", path.display());

            // Say whether the rewrite actually helped, with the real compiler.
            let opts = CheckOptions::default();
            match detect_env(&opts.packages) {
                Ok(env) => {
                    let file_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "snippet.cpp".to_string());
                    let report = check_with_env(&rewritten, &file_name, &opts, &env);
                    if report.success {
                        println!("✓ the rewritten file now compiles (Qt {})", report.qt_version);
                        ExitCode::SUCCESS
                    } else {
                        println!("✗ still failing after the rewrite:");
                        for diagnostic in report.diagnostics.iter().take(5) {
                            println!(
                                "  {} {}: {}",
                                diagnostic.severity,
                                diagnostic.line.unwrap_or(0),
                                diagnostic.message
                            );
                        }
                        ExitCode::from(1)
                    }
                }
                Err(e) => {
                    eprintln!("qtcheck: rewrote the file but cannot verify it: {e}");
                    ExitCode::from(4)
                }
            }
        }

        Cmd::Project { pro, json } => {
            let spec = match qtcheck::project::parse(&pro) {
                Ok(spec) => spec,
                Err(e) => {
                    eprintln!("qtcheck: {e}");
                    return ExitCode::from(4);
                }
            };
            let opts = CheckOptions {
                std_version: spec.std_version.clone(),
                packages: spec.packages.clone(),
                include_dirs: spec.include_dirs.clone(),
                timeout_ms: 20_000,
            };
            let env = match detect_env(&opts.packages) {
                Ok(env) => env,
                Err(e) => {
                    eprintln!("qtcheck: {e}");
                    return ExitCode::from(4);
                }
            };

            let mut results = Vec::new();
            let mut failed = 0usize;
            for file in &spec.files {
                let file_name = file
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "snippet.cpp".to_string());
                let report = match std::fs::read_to_string(file) {
                    Ok(code) => check_with_env(&code, &file_name, &opts, &env),
                    Err(e) => {
                        failed += 1;
                        if !json {
                            println!("✗ {} — cannot read: {e}", file.display());
                        }
                        results.push(serde_json::json!({
                            "path": file.display().to_string(),
                            "success": false,
                            "error": e.to_string(),
                        }));
                        continue;
                    }
                };
                if !report.success {
                    failed += 1;
                }
                if !json {
                    let mark = if report.success { "✓" } else { "✗" };
                    let moc = if report.moc_ran { ", moc" } else { "" };
                    println!(
                        "{mark} {}  ({} ms{moc})",
                        file.display(),
                        report.duration_ms.round()
                    );
                    for diagnostic in report.diagnostics.iter().filter(|d| d.severity != "warning").take(3) {
                        println!(
                            "    {} {}: {}",
                            diagnostic.severity,
                            diagnostic.line.unwrap_or(0),
                            diagnostic.message
                        );
                    }
                }
                results.push(serde_json::json!({
                    "path": file.display().to_string(),
                    "success": report.success,
                    "duration_ms": report.duration_ms.round(),
                    "moc_ran": report.moc_ran,
                    "diagnostics": report.diagnostics,
                }));
            }

            let total = spec.files.len();
            if json {
                let payload = serde_json::json!({
                    "project": pro.display().to_string(),
                    "name": spec.name,
                    "qt_version": env.qt_version,
                    "packages": spec.packages,
                    "std": spec.std_version,
                    "total": total,
                    "failed": failed,
                    "files": results,
                });
                match serde_json::to_string_pretty(&payload) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("qtcheck: cannot serialize report: {e}");
                        return ExitCode::from(4);
                    }
                }
            } else {
                println!();
                println!(
                    "{}: {}/{} files clean (Qt {}, {})",
                    spec.name,
                    total - failed,
                    total,
                    env.qt_version,
                    spec.std_version
                );
            }

            if failed == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }

        Cmd::Doctor => {
            let packages: Vec<String> = DEFAULT_PACKAGES.iter().map(|p| p.to_string()).collect();
            match detect_env(&packages) {
                Ok(env) => {
                    println!("Qt:       {}", env.qt_version);
                    println!("moc:      {}", env.moc.display());
                    println!("compiler: {}", env.compiler);
                    println!("include flags: {}", env.cflags.len());
                    let report = check_with_env(
                        "#include <QString>\nQString canary() { return QStringLiteral(\"ok\"); }\n",
                        "canary.cpp",
                        &CheckOptions::default(),
                        &env,
                    );
                    println!(
                        "canary:   {} ({} ms)",
                        report.status(),
                        report.duration_ms.round()
                    );
                    if report.success {
                        ExitCode::SUCCESS
                    } else {
                        for diagnostic in &report.diagnostics {
                            println!("  {}: {}", diagnostic.severity, diagnostic.message);
                        }
                        ExitCode::from(1)
                    }
                }
                Err(e) => {
                    println!("Qt: NOT USABLE — {e}");
                    ExitCode::from(4)
                }
            }
        }
    }
}
