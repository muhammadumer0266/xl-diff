# Rust + PyO3 Integration Plan (concise)

Goal: implement a high-performance Rust core to compute cell-level diffs across multiple sheets, expose a safe Python API via PyO3/maturin, and package wheels for PyPI.

1) Design Rust API surface
  - `compare_workbooks(left_path: &str, right_path: &str, key: Option<&str>) -> JsValue` where JsValue is JSON serializable delta map: { sheet_name: [CellDelta...] }
  - `CellDelta` struct: coordinate, old_value, new_value, change_type, formula_opt

2) Sheet reading
  - Use `calamine` to stream sheets; avoid materializing entire workbook when possible.
  - Implement row fingerprint hashing to support LCS alignment for large sheets.

3) Alignment & diff
  - Implement keyed-index matching or Myers/LCS fingerprint for positional fallback.
  - Parallelize per-sheet diff with `rayon` threadpool; keep CPU affinity conservative for server usage.

4) Memory safety & panics
  - Wrap heavy calls with `std::panic::catch_unwind` before crossing the FFI boundary.
  - Return Python exceptions via `PyErr::new::<PyRuntimeError, _>(...)` on error.

5) PyO3 binding
  - Expose `compare_workbooks_py` function using `py.allow_threads` and return `PyObject` JSON string or native Python list/dict via `serde_json` and `pyo3-serde`.

6) Packaging & CI
  - Use `maturin` in GitHub Actions to build manylinux wheels, and musllinux for Alpine targets.
  - Add `cibuildwheel` if native wheel matrices required.

7) Testing & fuzzing
  - Reuse `EDGE_CASES.md` to build unit tests for format/structure edge cases.
  - Add property-based tests (Hypothesis in Python or proptest in Rust) for monkey fuzzing.
  - Add memory and leak checks in CI using Valgrind or ASAN builds where feasible.

8) Performance tuning
  - Benchmark I/O vs compute; memory-map large files and process streaming XML for XLSX.
  - Avoid large string allocations in hot paths; return indices and reconstruct lightweight coords.

9) API compatibility
  - Keep the Python API backward compatible: `diff_sheets` falls back to Python implementation until Rust is available.
  - Provide a configuration flag to enable Rust backend when present.

10) Rollout
  - Publish Rust-backed wheels as a minor release, with a pure-Python fallback for platforms without binary wheels.
