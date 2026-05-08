# Building Editable From Source

Editable is a Rust-only macOS CSV document app. The UI talks to AppKit through
Rust `objc2` bindings, and the CSV engine lives in `editable-csv-core`. The app
uses native `NSTableView` editing while keeping parsing, sorting, filtering,
row indexing, edit overlays, and saving in Rust.

This repository intentionally does not include a README yet.

## Requirements

- macOS with Apple Command Line Tools or Xcode.
- Rust stable, installed through `rustup`.
- The standard macOS tools `sips` and `iconutil` for icon generation.

Check the Apple tools:

```bash
xcode-select --install
clang --version
which sips
which iconutil
```

Install Rust if `cargo --version` fails:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup default stable
```

## Build

From the repository root:

```bash
cargo build --release -p editable
```

Run the test suite:

```bash
cargo test
```

For a quick debug launch from source with the bundled sample:

```bash
cargo run -p editable
```

For a quick debug launch with your own CSV:

```bash
cargo run -p editable -- /path/to/file.csv
```

## Generate The App Icon

The generated source image is stored at:

```text
assets/icon/editable-icon.png
```

Generate the iconset and `.icns` file:

```bash
cargo run --release -p editable-build-tools -- icon
```

The expected output is:

```text
assets/icon/Editable.icns
```

If `iconutil` rejects an iconset, remove `assets/icon/Editable.iconset`, rerun
the command, and verify that all PNGs are square RGB or RGBA images at the
expected sizes.

## Bundle The macOS App

After a release build and icon generation:

```bash
cargo run --release -p editable-build-tools -- bundle
```

The expected output is:

```text
dist/Editable.app
```

You can do the full local build in one command:

```bash
./scripts/build-app.sh
```

## Launch Locally

Launch the bundled app:

```bash
open dist/Editable.app
```

Run the binary directly with a CSV file:

```bash
target/release/editable assets/samples/basic.csv
```

## Source-Level Verification

After launching a CSV, verify the CSV-only editor workflow:

- Toggle `Header` to decide whether the first non-skipped row is column names.
- Enter a number in `Skip` and press Return to reopen the file while ignoring
  that many leading rows in the grid.
- Click a cell and type to edit it. Double-click a cell to start editing
  immediately.
- Select rows or columns in the table, then use `Delete` to clear selected
  cells or remove selected rows/columns.
- Use `+ Row`, `+ Col`, `Row Up`, `Row Down`, `Col Left`, and `Col Right` for
  insertion and reordering.
- Use `A-Z`, `Z-A`, and the filter field for active-column sort and contains
  filtering.
- Use `Save`, then reopen the file to confirm edits, ordering, skipped leading
  rows, and CSV quoting round-trip correctly.

## Optional Local Codesign

For local testing outside the terminal:

```bash
codesign --force --deep --sign - dist/Editable.app
```

## Manual Verification

- Confirm there is no `README.md`.
- Confirm `dist/Editable.app/Contents/Info.plist` has bundle identifier
  `dev.local.editable`.
- Open a `.csv` file from Finder or pass one as an argument.
- Verify the app displays only CSV/table functionality.
- Edit, insert, delete, sort, filter, save, and reopen through the source-level
  workflows before packaging a release.
