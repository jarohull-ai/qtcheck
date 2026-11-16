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
# check a file, or a shell glob of files (checked in parallel)
qtcheck check widget.cpp
qtcheck check src/*.cpp

# check stdin, machine-readable
echo '#include <QString>
int f() { return QString("x"); }' | qtcheck check --json

# the Qt 5 leftovers in a file, with what to write instead
qtcheck port legacy.cpp

# check a whole project: modules, sources and headers come from the project file
qtcheck project myapp.pro
qtcheck project CMakeLists.txt

# the migration job: every leftover in the project, then rewrite the safe ones
qtcheck port --project myapp.pro
qtcheck port --project myapp.pro --write

# rewrite the safe renames in one file and re-verify it with the compiler
qtcheck port --write legacy.cpp

# what Qt will be used
qtcheck doctor
```

`check` flags: `--std c++17`, `--package Qt6Charts` (repeatable), `--include
src/` (repeatable), `--timeout-ms 10000`, `--json`, `--github`.
`project` flags: `--jobs N` (`0` = one per CPU), `--json`, `--github`.

### Speed

Files are independent compilations, so `project` and multi-file `check` run one
per CPU. Measured on this machine (12 threads, Qt 6.12.0, release build):

| project | files | `--jobs 1` | default |
|---|---|---|---|
| astramon | 5 | 6.59 s | **1.67 s** |
| xcratesx | 7 | 9.42 s | **2.10 s** |

One file costs 0.7–1.7 s, and 32 ms of that is the tool — the rest is the C++
compiler reading Qt headers.

### In CI

`--github` prints [workflow commands](https://docs.github.com/actions/using-workflows/workflow-commands-for-github-actions)
instead of a report, so failures appear on the changed line of the pull request:

```yaml
- name: Check Qt code
  run: |
    cargo install --path . --locked
    qtcheck project CMakeLists.txt --github
```

A clean project prints nothing and exits 0. The exit code still distinguishes a
broken project (1) from a broken toolchain (4), so a missing Qt does not look
like a code failure.

`project` accepts a qmake `.pro` file or a `CMakeLists.txt`, takes the modules,
sources, headers, include directories and C++ standard from it, and checks every
file, running `moc` for the headers that declare a meta-object.

- **qmake**: `QT`, `SOURCES`, `HEADERS`, `INCLUDEPATH`, `CONFIG += c++NN`,
  `TARGET`.
- **CMake**: `find_package(Qt6 COMPONENTS ...)`, `target_link_libraries(...
  Qt6::X)`, `add_executable` / `add_library` / `qt_add_executable` /
  `target_sources` / `qt_add_qml_module`, `target_include_directories`,
  `set(CMAKE_CXX_STANDARD ...)`, and simple `${VARIABLE}` expansion (including
  CMake's own `${CMAKE_CURRENT_SOURCE_DIR}`).

Neither is a build system: conditions, `include()`, functions and generator
expressions are ignored, and unknown pieces are dropped rather than guessed. If
the parsed file list looks wrong, the project file is more than this tool
understands — use the build system instead.

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
- **Headers are checked as if they were sources.** A header that needs a macro
  or an include only the build system provides will look broken here; pass
  `--include` for its directory, or check the `.cpp` that uses it.
- **One translation unit at a time.** It type-checks syntax against Qt headers; it does not resolve
  headers you do not pass with `--include`, and it does not link, so missing
  definitions are not reported.
- **Translation units, not projects.** `project` checks files one by one, not
  the build: it uses no object files, so linking and missing definitions are
  out of scope. For a real build, use CMake or qmake.
- **`--jobs` trades memory for speed.** Each worker is a compiler process over
  Qt headers; on a small machine `--jobs 2` may be faster than thrashing.
- **Not every CMake is understood.** `qt_add_qml_module` QML files are skipped
  (no C++ compiler can check QML), and a project whose sources come from a
  function call or a generator expression will be reported as having none.
- Verified on Qt 6.12.0 / g++ 16.2.1 / x86_64 Linux only.

## Licence

MIT — see `LICENSE`.
