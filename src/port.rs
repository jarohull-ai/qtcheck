//! Qt 5 spellings that Qt 6 removed, with what to write instead.
//!
//! Every entry carries a probe pair: code using the old spelling and code using
//! the replacement. `tests/port_table.rs` compiles both against the Qt that is
//! installed, so an entry can only stay in this table while the claim it makes
//! is true — old code fails, new code compiles. If Qt changes again, the table
//! breaks loudly instead of quietly giving wrong advice.
//!
//! `auto` marks a bare token rename that is safe to apply mechanically.
//! Everything else needs a human: `QRegExp` is not `QRegularExpression` with a
//! different name, it is a different API.

use serde::Serialize;

/// One Qt 5 → Qt 6 migration rule.
pub struct Entry {
    /// Text as it appears in Qt 5 code.
    pub needle: &'static str,
    /// What to write in Qt 6.
    pub replacement: &'static str,
    /// One sentence a developer can act on.
    pub note: &'static str,
    /// Safe to rewrite without reading the surrounding code.
    pub auto: bool,
    /// Skip the match when the text before it ends with one of these
    /// (`std::endl` is not `endl`).
    pub guards: &'static [&'static str],
    /// `(Qt 5 code, Qt 6 code)` — the proof, used by the test suite.
    pub probe: (&'static str, &'static str),
}

/// Rules verified against Qt 6.12.0 on this machine.
pub const ENTRIES: &[Entry] = &[
    Entry {
        needle: "QString::SkipEmptyParts",
        replacement: "Qt::SkipEmptyParts",
        note: "Qt 6 moved the split flags out of QString into the Qt namespace.",
        auto: true,
        guards: &[],
        probe: (
            "#include <QString>\n#include <QStringList>\nQStringList split_it() { return QString(\"a,b\").split(',', QString::SkipEmptyParts); }\n",
            "#include <QString>\n#include <QStringList>\nQStringList split_it() { return QString(\"a,b\").split(',', Qt::SkipEmptyParts); }\n",
        ),
    },
    Entry {
        needle: "QString::KeepEmptyParts",
        replacement: "Qt::KeepEmptyParts",
        note: "Same move as SkipEmptyParts: the flag now lives in Qt, not QString.",
        auto: true,
        guards: &[],
        probe: (
            "#include <QString>\n#include <QStringList>\nQStringList split_it() { return QString(\"a,b\").split(',', QString::KeepEmptyParts); }\n",
            "#include <QString>\n#include <QStringList>\nQStringList split_it() { return QString(\"a,b\").split(',', Qt::KeepEmptyParts); }\n",
        ),
    },
    Entry {
        needle: "endl",
        replacement: "Qt::endl",
        note: "endl is in the Qt namespace in Qt 6; std::endl flushes differently and works.",
        auto: true,
        guards: &["std::", "Qt::"],
        probe: (
            "#include <QDebug>\nvoid log_it() { qDebug() << \"hello\" << endl; }\n",
            "#include <QDebug>\nvoid log_it() { qDebug() << \"hello\" << Qt::endl; }\n",
        ),
    },
    Entry {
        needle: "QStandardPaths::DataLocation",
        replacement: "QStandardPaths::AppDataLocation",
        note: "DataLocation was removed; AppDataLocation is the replacement in Qt 6.",
        auto: true,
        guards: &[],
        probe: (
            "#include <QStandardPaths>\nQString data_dir() { return QStandardPaths::writableLocation(QStandardPaths::DataLocation); }\n",
            "#include <QStandardPaths>\nQString data_dir() { return QStandardPaths::writableLocation(QStandardPaths::AppDataLocation); }\n",
        ),
    },
    Entry {
        needle: "QRegExp",
        replacement: "QRegularExpression",
        note: "QRegExp is gone from Qt 6. QRegularExpression is a different API: match() returns QRegularExpressionMatch, so check each call site.",
        auto: false,
        guards: &[],
        probe: (
            "#include <QRegExp>\nbool matches() { QRegExp re(\"a+\"); return re.exactMatch(\"aaa\"); }\n",
            "#include <QRegularExpression>\nbool matches() { QRegularExpression re(\"a+\"); return re.match(\"aaa\").hasMatch(); }\n",
        ),
    },
    Entry {
        needle: "QDesktopWidget",
        replacement: "QScreen",
        note: "QDesktopWidget was removed; ask QGuiApplication::primaryScreen() or QGuiApplication::screens().",
        auto: false,
        guards: &[],
        probe: (
            "#include <QDesktopWidget>\nint screen_width() { QDesktopWidget widget; return widget.width(); }\n",
            "#include <QGuiApplication>\n#include <QScreen>\nint screen_width() { return QGuiApplication::primaryScreen()->geometry().width(); }\n",
        ),
    },
    Entry {
        needle: "qrand()",
        replacement: "QRandomGenerator::global()->bounded(...)",
        note: "qrand() was removed; QRandomGenerator is the Qt 6 random source.",
        auto: false,
        guards: &[],
        probe: (
            "#include <QtGlobal>\nint roll() { return qrand() % 6; }\n",
            "#include <QRandomGenerator>\nint roll() { return int(QRandomGenerator::global()->bounded(6)); }\n",
        ),
    },
    Entry {
        needle: "setCodec(",
        replacement: "setEncoding(",
        note: "setCodec took a name in Qt 5; Qt 6 takes a QStringConverter::Encoding.",
        auto: false,
        guards: &[],
        probe: (
            "#include <QTextStream>\nvoid configure(QTextStream &stream) { stream.setCodec(\"UTF-8\"); }\n",
            "#include <QTextStream>\nvoid configure(QTextStream &stream) { stream.setEncoding(QStringConverter::Utf8); }\n",
        ),
    },
    Entry {
        needle: "QString::null",
        replacement: "QString()",
        note: "QString::null was removed; a default-constructed QString is the null string.",
        auto: false,
        guards: &[],
        probe: (
            "#include <QString>\nQString empty_text() { QString text = QString::null; return text; }\n",
            "#include <QString>\nQString empty_text() { return QString(); }\n",
        ),
    },
];

/// One occurrence found in a source file.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub line: u32,
    pub column: u32,
    pub needle: String,
    pub replacement: String,
    pub note: String,
    pub auto: bool,
    #[serde(skip)]
    pub byte_start: usize,
    #[serde(skip)]
    pub byte_end: usize,
}

impl Hit {
    /// `12:5  QString::SkipEmptyParts -> Qt::SkipEmptyParts  (auto)`
    pub fn describe(&self) -> String {
        format!(
            "{}:{}  {} -> {}  ({})",
            self.line,
            self.column,
            self.needle,
            self.replacement,
            if self.auto { "auto" } else { "manual" }
        )
    }
}

/// Find every Qt 5 spelling in `code`, longest match first at the same offset.
pub fn scan(code: &str) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    for entry in ENTRIES {
        for (offset, _) in code.match_indices(entry.needle) {
            if !at_token_boundary(code, offset, entry.needle.len()) {
                continue;
            }
            let before = &code[..offset];
            if entry.guards.iter().any(|guard| before.ends_with(guard)) {
                continue;
            }
            let end = offset + entry.needle.len();
            if hits
                .iter()
                .any(|seen| offset < seen.byte_end && end > seen.byte_start)
            {
                continue;
            }
            let (line, column) = line_column(before);
            hits.push(Hit {
                line,
                column,
                needle: entry.needle.to_string(),
                replacement: entry.replacement.to_string(),
                note: entry.note.to_string(),
                auto: entry.auto,
                byte_start: offset,
                byte_end: end,
            });
        }
    }
    hits.sort_by_key(|hit| hit.byte_start);
    hits
}

/// Apply the automatic rewrites, from the end so earlier offsets stay valid.
pub fn apply(code: &str, hits: &[Hit]) -> (String, usize) {
    let mut ordered: Vec<&Hit> = hits.iter().filter(|hit| hit.auto).collect();
    ordered.sort_by_key(|hit| std::cmp::Reverse(hit.byte_start));

    let mut out = code.to_string();
    let mut applied = 0usize;
    let mut boundary = usize::MAX;
    for hit in ordered {
        if hit.byte_end > boundary {
            continue;
        }
        out.replace_range(hit.byte_start..hit.byte_end, &hit.replacement);
        boundary = hit.byte_start;
        applied += 1;
    }
    (out, applied)
}

fn at_token_boundary(code: &str, offset: usize, len: usize) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let before_ok = code[..offset].chars().next_back().map(|c| !ident(c)).unwrap_or(true);
    let after_ok = code[offset + len..].chars().next().map(|c| !ident(c)).unwrap_or(true);
    before_ok && after_ok
}

fn line_column(prefix: &str) -> (u32, u32) {
    let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32 + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .map(|tail| tail.chars().count() as u32 + 1)
        .unwrap_or(1);
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_pure_rename_with_position() {
        let code = "#include <QStringList>\nQStringList parts = text.split(',', QString::SkipEmptyParts);\n";
        let hits = scan(code);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 2);
        assert_eq!(hits[0].needle, "QString::SkipEmptyParts");
        assert!(hits[0].auto);
    }

    #[test]
    fn does_not_touch_std_endl_or_qt_endl() {
        assert!(scan("std::cout << std::endl;").is_empty());
        assert!(scan("qDebug() << Qt::endl;").is_empty());
        assert_eq!(scan("qDebug() << endl;").len(), 1);
    }

    #[test]
    fn respects_identifier_boundaries() {
        assert!(scan("int myqrand() { return 1; }").is_empty());
        assert_eq!(scan("int x = qrand();").len(), 1);
    }

    #[test]
    fn apply_rewrites_only_automatic_hits() {
        let code = "QStringList p = s.split(',', QString::SkipEmptyParts);\nQRegExp re(\"a\");\n";
        let hits = scan(code);
        assert_eq!(hits.len(), 2);
        let (out, applied) = apply(code, &hits);
        assert_eq!(applied, 1);
        assert!(out.contains("Qt::SkipEmptyParts"), "out: {out}");
        assert!(out.contains("QRegExp"), "manual entries must stay: {out}");
    }
}
