# xl-diff

[![CI](https://github.com/muhammadumer0266/xl-diff/actions/workflows/CI.yml/badge.svg)](https://github.com/muhammadumer0266/xl-diff/actions/workflows/CI.yml)
[![crates.io](https://img.shields.io/crates/v/xl_diff.svg)](https://crates.io/crates/xl_diff)
[![PyPI](https://img.shields.io/pypi/v/xl-diff.svg)](https://pypi.org/project/xl-diff/)
[![Language](https://img.shields.io/badge/Language-Rust%20%2F%20Python-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

xl-diff is a high-performance, memory-safe Excel comparison engine written in Rust and exposed as a Python extension via PyO3. It is designed to integrate into Django/Celery pipelines and produce cell-level semantic diffs between spreadsheet versions.

Features
- Fast, zero-copy reading of XLSX using `calamine`.
- Parallel diff computation using `rayon`.
- Row alignment using an optional key column or positional fallback.
- Python bindings (PyO3) for seamless integration.
- Cross-platform packaging with `maturin` and GitHub Actions.

Quick start (developer)

```bash
# Clone
git clone https://github.com/muhammadumer0266/xl-diff.git
cd xl-diff

# Create virtualenv
python -m venv .venv
source .venv/bin/activate

# Install maturin and build the extension in-place
pip install maturin
maturin develop --release
```

Python usage

```python
import xl_diff

# List sheets
sheets = xl_diff.get_sheet_names("/path/to/file.xlsx")

# Diff two sheets by key column 0
deltas = xl_diff.diff_sheets(
    "/path/to/old.xlsx",
    "Sheet1",
    "/path/to/new.xlsx",
    "Sheet1",
    0,
)

for d in deltas:
    print(d.row_idx_old, d.row_idx_new, d.col_idx, d.status, d.old_value, d.new_value)
```

Badge & CI

This repository includes GitHub Actions workflows to build manylinux/musllinux and wheels for Windows/macOS, along with an sdist job. The CI badge above links to the main workflow.

Contributing

Please open issues for bugs and feature requests. Pull requests should target the `main` branch; the `issue/traceability-phase2` branch contains traceability and test improvements.

License

MIT
