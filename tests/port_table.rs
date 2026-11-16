//! Proof, not assertion: every entry in the migration table is compiled both
//! ways against the Qt that is installed here. An entry survives only while
//! the old spelling fails and the replacement compiles.

use qtcheck::{check_with_env, detect_env, port, CheckOptions, DEFAULT_PACKAGES};

fn env() -> qtcheck::QtEnv {
    let packages: Vec<String> = DEFAULT_PACKAGES.iter().map(|p| p.to_string()).collect();
    detect_env(&packages).unwrap_or_else(|e| panic!("these tests need Qt 6 development files: {e}"))
}

#[test]
fn every_entry_is_a_real_qt6_break() {
    let env = env();
    let opts = CheckOptions::default();
    let mut checked = 0;

    for entry in port::ENTRIES {
        let old = check_with_env(entry.probe.0, "old.cpp", &opts, &env);
        assert!(
            !old.success,
            "`{}` still compiles under Qt {} — the entry is obsolete and must be removed",
            entry.needle, env.qt_version
        );

        let new = check_with_env(entry.probe.1, "new.cpp", &opts, &env);
        assert!(
            new.success,
            "the replacement for `{}` does not compile under Qt {}:\n{}",
            entry.needle, env.qt_version, new.stderr
        );
        checked += 1;
    }

    assert!(checked >= 5, "table shrank to {checked} entries; that is suspicious");
}

#[test]
fn every_entry_is_scannable() {
    // A rule nobody can find is documentation, not a check: the scanner has to
    // hit the Qt 5 probe on its own, without knowing where to look.
    for entry in port::ENTRIES {
        let hits = port::scan(entry.probe.0);
        assert!(
            hits.iter().any(|hit| hit.needle == entry.needle),
            "the scanner does not find `{}` in its own probe (found {:?})",
            entry.needle,
            hits.iter().map(|h| h.needle.clone()).collect::<Vec<_>>()
        );
        assert_ne!(entry.needle, entry.replacement, "{} maps to itself", entry.needle);
    }
}
