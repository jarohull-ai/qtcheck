# qtcheck

Is this Qt 6 code real, or does it only look like Qt?

```bash
$ qtcheck check widget.cpp
✗ fail  (238 ms, Qt 6.12.0)
  error 12:45: 'SkipEmptyParts' is not a member of 'QString'

  1 Qt 5 spelling(s) found — run `qtcheck port` for the Qt 6 replacements
```

## The problem

Code written against Qt 5 — by a person upgrading a project, or by an assistant
that learned from older examples — fails in ways that are slow to diagnose:
`QString::SkipEmptyParts` does not exist, `QRegExp` was deleted outright,
`QFontMetrics::width()` is gone. Finding out by building the project means
minutes of template noise to discover one renamed flag.

`qtcheck` answers in a few hundred milliseconds, against **your** Qt headers:

- compiles one translation unit with the include flags `pkg-config` reports;
- runs `moc` when the source declares a meta-object (`Q_OBJECT`, `Q_GADGET`,
  `Q_NAMESPACE`), so `#include "widget.moc"` resolves;
- parses compiler output into structured diagnostics (line, column, severity);
- names the Qt 5 spellings that Qt 6 removed, and the replacement.

## Install

```bash
cargo install --path .          # or: cargo build --release, then copy the binary
```

Needs a C++ compiler, `pkg-config` and Qt 6 development files
(`qt6-base-dev` on Debian/Ubuntu, `qt6-base` on Arch).

## Use

```bash
# check a file
qtcheck check widget.cpp

# check stdin, machine-readable
echo '#include <QString>
int f() { return QString("x"); }' | qtcheck check --json

# the Qt 5 leftovers in a file, with what to write instead
qtcheck port legacy.cpp

# check a whole qmake project: modules, sources and headers come from the .pro
qtcheck project myapp.pro

# rewrite the safe renames and re-verify the result with the compiler
qtcheck port --write legacy.cpp

# what Qt will be used
qtcheck doctor
```

`check` flags: `--std c++17`, `--package Qt6Charts` (repeatable), `--include
src/` (repeatable), `--timeout-ms 10000`, `--json`.

`project` reads a subset of qmake — `QT`, `SOURCES`, `HEADERS`, `INCLUDEPATH`,
`CONFIG += c++NN`, `TARGET` — resolves the paths against the `.pro` directory,
and checks every file, running `moc` for the headers that declare a
meta-object. It is not qmake: conditions, `include()` and computed values are
ignored, and CMake is not supported yet.

## What it knows

Each rule below is verified by the test suite: the Qt 5 code must fail to
compile and the replacement must compile, against the Qt installed on the
machine running the tests. If a rule stops being true, the suite fails instead
of the tool quietly giving wrong advice.

| Qt 5 | Qt 6 | rewrite |
|---|---|---|
| `QString::SkipEmptyParts` | `Qt::SkipEmptyParts` | automatic |
| `QString::KeepEmptyParts` | `Qt::KeepEmptyParts` | automatic |
| `endl` (not `std::endl`) | `Qt::endl` | automatic |
| `QStandardPaths::DataLocation` | `AppDataLocation` | automatic |
| `QRegExp` | `QRegularExpression` | manual — different API |
| `QDesktopWidget` | `QScreen` | manual |
| `qrand()` | `QRandomGenerator` | manual |
| `setCodec(` | `setEncoding(` | manual — takes an encoding, not a name |
| `QString::null` | `QString()` | manual |

Only text that can be found without guessing types is in the table. A member
call like `fm.width("...")` is *not* listed: matching `.width(` would also hit
`QWidget::width` and `QRect::width`. For those, the compiler error is the
signal — `no member named 'width' in 'QFontMetrics'` — and `horizontalAdvance`
is the answer.

## Exit codes

`0` the code is fine · `1` the code is wrong (or `port` found leftovers) ·
`3` timeout · `4` the tool cannot work (no Qt, no compiler, no moc) ·
`64` bad command line.

That separation is deliberate: a pipeline must be able to tell "this code does
not compile" from "this machine has no Qt".

## Limits

- **Qt 6 only.** No Qt 5 mode.
- **One file.** It type-checks syntax against Qt headers; it does not resolve
  headers you do not pass with `--include`, and it does not link, so missing
  definitions are not reported.
- **Translation units, not projects.** `project` checks files one by one, not
  the build: it uses no object files, so linking and missing definitions are
  out of scope. For a real build, use CMake or qmake.
- Verified on Qt 6.12.0 / g++ 16.2.1 / x86_64 Linux only.

## Licence

MIT — see `LICENSE`.
