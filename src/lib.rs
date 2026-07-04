// src/lib.rs
use pyo3::prelude::*;
use pyo3::wrap_pyfunction;
use pyo3::exceptions::PyRuntimeError;
use calamine::{Reader, open_workbook_auto, Data};
use std::path::Path;
use std::collections::{HashMap, HashSet};
use rayon::prelude::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::fs;
use std::env;
use serde::Serialize;

// For stable, fast row fingerprints used by LCS
use blake3;
use hex;

// Default limits (can be overridden via environment variables)
const DEFAULT_MAX_INPUT_BYTES: u64 = 200 * 1024 * 1024; // 200 MB
const DEFAULT_MAX_CELL_COUNT: usize = 10_000_000; // 10 million cells
const DEFAULT_LCS_MAX_PAIRWISE: usize = 5_000_000; // n*m threshold to skip LCS

fn env_u64(name: &str, fallback: u64) -> u64 {
    env::var(name).ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(fallback)
}

fn env_usize(name: &str, fallback: usize) -> usize {
    env::var(name).ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(fallback)
}

// Define the cell delta structure that will be exposed to Python
#[pyclass]
#[derive(Serialize)]
#[derive(Clone, Debug)]
pub struct CellDelta {
    #[pyo3(get)]
    pub row_idx_old: Option<usize>,
    #[pyo3(get)]
    pub row_idx_new: Option<usize>,
    #[pyo3(get)]
    pub col_idx: usize,
    #[pyo3(get)]
    pub old_value: String,
    #[pyo3(get)]
    pub new_value: String,
    #[pyo3(get)]
    pub status: String, // "Modified", "Added", "Deleted"
}

#[derive(Clone, Debug, Serialize)]
pub struct SheetDiffReport {
    pub sheet_name: String,
    pub status: String,
    pub row_count_old: Option<usize>,
    pub row_count_new: Option<usize>,
    pub cell_deltas: Vec<CellDelta>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkbookDiffSummary {
    pub total_sheets_old: usize,
    pub total_sheets_new: usize,
    pub compared_sheets: usize,
    pub added_sheets: usize,
    pub deleted_sheets: usize,
    pub changed_sheets: usize,
    pub changed_cells: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkbookDiffReport {
    pub old_file: String,
    pub new_file: String,
    pub selected_sheets: Vec<String>,
    pub summary: WorkbookDiffSummary,
    pub sheets: Vec<SheetDiffReport>,
}

// Internal enum to track row structural alignments
#[derive(Clone, Debug)]
enum RowAlignment {
    Matched(usize, usize), // (old_row_idx, new_row_idx)
    Deleted(usize),        // (old_row_idx)
    Added(usize),          // (new_row_idx)
}

// Helper function to safely stringify calamine cell variants
fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Int(i) => i.to_string(),
        Data::Float(f) => f.to_string(),
        Data::String(s) => s.trim().to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => d.to_string(),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("{:?}", e),
        Data::Empty => String::new(),
    }
}

// Compare two Data values with a tolerance for numeric types
fn data_equal_with_tolerance(a: &Data, b: &Data, eps: f64) -> bool {
    match (a, b) {
        (Data::Float(x), Data::Float(y)) => (x - y).abs() <= eps,
        (Data::Float(x), Data::Int(i)) => (x - (*i as f64)).abs() <= eps,
        (Data::Int(i), Data::Float(x)) => ((*i as f64) - x).abs() <= eps,
        (Data::Int(i), Data::Int(j)) => i == j,
        (Data::String(s1), Data::String(s2)) => s1.trim() == s2.trim(),
        (Data::Bool(b1), Data::Bool(b2)) => b1 == b2,
        (Data::Empty, Data::Empty) => true,
        // Fallback to string compare for mixed types
        _ => cell_to_string(a) == cell_to_string(b),
    }
}

// Trim trailing empty rows (completely empty) to avoid padding blowups
fn trim_trailing_empty_rows(mut rows: Vec<Vec<Data>>) -> Vec<Vec<Data>> {
    while let Some(last) = rows.last() {
        let all_empty = last.iter().all(|c| matches!(c, Data::Empty));
        if all_empty {
            rows.pop();
        } else {
            break;
        }
    }
    rows
}

fn list_sheet_names<P: AsRef<Path>>(file_path: P) -> Result<Vec<String>, String> {
    let workbook = open_workbook_auto(file_path).map_err(|e| e.to_string())?;
    Ok(workbook.sheet_names())
}

fn selected_sheet_filter(selected_sheets: Option<Vec<String>>) -> Option<HashSet<String>> {
    selected_sheets.map(|items| items.into_iter().collect())
}

fn should_compare_sheet(sheet_name: &str, selected: Option<&HashSet<String>>) -> bool {
    selected.map_or(true, |set| set.contains(sheet_name))
}

fn compare_workbooks_report(
    old_file: &str,
    new_file: &str,
    key_index: Option<usize>,
    selected_sheets: Option<Vec<String>>,
) -> Result<WorkbookDiffReport, String> {
    let old_sheet_names = list_sheet_names(old_file)?;
    let new_sheet_names = list_sheet_names(new_file)?;
    let selected_lookup = selected_sheet_filter(selected_sheets);
    let selected_sheet_list = selected_lookup
        .as_ref()
        .map(|set| set.iter().cloned().collect::<Vec<String>>())
        .unwrap_or_default();

    let old_set: HashSet<String> = old_sheet_names.iter().cloned().collect();
    let new_set: HashSet<String> = new_sheet_names.iter().cloned().collect();

    let mut reports = Vec::new();
    let mut added_sheets = 0usize;
    let mut deleted_sheets = 0usize;
    let mut compared_sheets = 0usize;
    let mut changed_sheets = 0usize;
    let mut changed_cells = 0usize;

    for sheet_name in &old_sheet_names {
        if !new_set.contains(sheet_name) {
            if should_compare_sheet(sheet_name, selected_lookup.as_ref()) {
                reports.push(SheetDiffReport {
                    sheet_name: sheet_name.clone(),
                    status: "Deleted".to_string(),
                    row_count_old: None,
                    row_count_new: None,
                    cell_deltas: Vec::new(),
                });
            }
            deleted_sheets += 1;
        }
    }

    for sheet_name in &new_sheet_names {
        if !old_set.contains(sheet_name) {
            if should_compare_sheet(sheet_name, selected_lookup.as_ref()) {
                reports.push(SheetDiffReport {
                    sheet_name: sheet_name.clone(),
                    status: "Added".to_string(),
                    row_count_old: None,
                    row_count_new: None,
                    cell_deltas: Vec::new(),
                });
            }
            added_sheets += 1;
        }
    }

    for sheet_name in &old_sheet_names {
        if new_set.contains(sheet_name) && should_compare_sheet(sheet_name, selected_lookup.as_ref()) {
            let old_grid = load_sheet_matrix(old_file, sheet_name)?;
            let new_grid = load_sheet_matrix(new_file, sheet_name)?;
            let alignments = align_matrices(&old_grid, &new_grid, key_index);
            let cell_deltas = compute_deltas_parallel(&old_grid, &new_grid, &alignments);
            if !cell_deltas.is_empty() {
                changed_sheets += 1;
                changed_cells += cell_deltas.len();
            }
            compared_sheets += 1;
            reports.push(SheetDiffReport {
                sheet_name: sheet_name.clone(),
                status: if cell_deltas.is_empty() {
                    "Unchanged".to_string()
                } else {
                    "Compared".to_string()
                },
                row_count_old: Some(old_grid.len()),
                row_count_new: Some(new_grid.len()),
                cell_deltas,
            });
        }
    }

    Ok(WorkbookDiffReport {
        old_file: old_file.to_string(),
        new_file: new_file.to_string(),
        selected_sheets: selected_sheet_list,
        summary: WorkbookDiffSummary {
            total_sheets_old: old_sheet_names.len(),
            total_sheets_new: new_sheet_names.len(),
            compared_sheets,
            added_sheets,
            deleted_sheets,
            changed_sheets,
            changed_cells,
        },
        sheets: reports,
    })
}

// Compute a fast fingerprint for a row using blake3 and return hex string
fn row_fingerprint_hex(row: &[Data]) -> String {
    let mut ctx = blake3::Hasher::new();
    for cell in row.iter() {
        let s = cell_to_string(cell);
        ctx.update(s.as_bytes());
        ctx.update(&[0x1f]); // separator
    }
    let out = ctx.finalize();
    hex::encode(&out.as_bytes()[0..8]) // short fingerprint
}

// Longest Common Subsequence (LCS) returning vector of matched index pairs
// Uses DP with O(n*m) time and memory; we guard execution with a threshold
fn lcs_matches(a: &[String], b: &[String], pairwise_limit: usize) -> Option<Vec<(usize, usize)>> {
    let n = a.len();
    let m = b.len();
    if n == 0 || m == 0 {
        return Some(Vec::new());
    }
    if (n as usize) * (m as usize) > pairwise_limit {
        // Too expensive
        return None;
    }
    // Build DP table of sizes (n+1)*(m+1) using u32 to save memory
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            if a[i] == b[j] {
                dp[i + 1][j + 1] = dp[i][j] + 1;
            } else {
                dp[i + 1][j + 1] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }
    // Backtrack to get matches
    let mut i = n;
    let mut j = m;
    let mut matches = Vec::new();
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            matches.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    matches.reverse();
    Some(matches)
}

// 1. Load Excel Sheet Matrix with trimming and defensive checks
fn load_sheet_matrix<P: AsRef<Path>>(file_path: P, sheet_name: &str) -> Result<Vec<Vec<Data>>, String> {
    // Get configured limits from env, or use defaults
    let max_input_bytes = env_u64("XL_DIFF_MAX_INPUT_BYTES", DEFAULT_MAX_INPUT_BYTES);
    let max_cell_count = env_usize("XL_DIFF_MAX_CELL_COUNT", DEFAULT_MAX_CELL_COUNT);

    let p = file_path.as_ref();
    if let Ok(meta) = fs::metadata(p) {
        if meta.len() > max_input_bytes {
            return Err(format!("Input file too large (> {} bytes)", max_input_bytes));
        }
    }

    let mut workbook = open_workbook_auto(&p).map_err(|e| e.to_string())?;

    if let Ok(range) = workbook.worksheet_range(sheet_name) {
        let rows = range.rows().map(|row| row.to_vec()).collect::<Vec<Vec<Data>>>();
        let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let cell_count = rows.len().saturating_mul(max_cols);
        if cell_count > max_cell_count {
            return Err(format!("Sheet has too many cells ({}) > limit {}", cell_count, max_cell_count));
        }
        Ok(trim_trailing_empty_rows(rows))
    } else {
        Err(format!("Sheet '{}' not found or failed to parse.", sheet_name))
    }
}

// 2. Align Matrices based on an optional Key Column Index
fn align_matrices(
    old_grid: &[Vec<Data>],
    new_grid: &[Vec<Data>],
    key_index: Option<usize>
) -> Vec<RowAlignment> {
    // If key_index present use map-based matching
    if let Some(idx) = key_index {
        let mut alignments = Vec::new();
        let mut old_map = HashMap::new();
        for (r_idx, row) in old_grid.iter().enumerate() {
            if let Some(cell_value) = row.get(idx) {
                let key_str = cell_to_string(cell_value);
                if !key_str.is_empty() {
                    old_map.insert(key_str, r_idx);
                }
            }
        }

        let mut matched_old = vec![false; old_grid.len()];

        for (new_idx, row) in new_grid.iter().enumerate() {
            if let Some(cell_value) = row.get(idx) {
                let key_str = cell_to_string(cell_value);
                if let Some(&old_idx) = old_map.get(&key_str) {
                    alignments.push(RowAlignment::Matched(old_idx, new_idx));
                    matched_old[old_idx] = true;
                } else {
                    alignments.push(RowAlignment::Added(new_idx));
                }
            } else {
                alignments.push(RowAlignment::Added(new_idx));
            }
        }

        for (old_idx, &matched) in matched_old.iter().enumerate() {
            if !matched {
                alignments.push(RowAlignment::Deleted(old_idx));
            }
        }

        return alignments;
    }

    // No key: attempt fingerprint + LCS alignment if affordable
    let pairwise_limit = env_usize("XL_DIFF_LCS_PAIRWISE_LIMIT", DEFAULT_LCS_MAX_PAIRWISE);
    let n = old_grid.len();
    let m = new_grid.len();
    // Create fingerprints
    let old_fps: Vec<String> = old_grid.iter().map(|r| row_fingerprint_hex(r)).collect();
    let new_fps: Vec<String> = new_grid.iter().map(|r| row_fingerprint_hex(r)).collect();

    if let Some(matches) = lcs_matches(&old_fps, &new_fps, pairwise_limit) {
        // Build alignments from matches, filling gaps as added/deleted
        let mut alignments = Vec::new();
        let mut oi = 0usize;
        let mut ni = 0usize;
        for (mo, mn) in matches.iter() {
            // all old rows before mo are deletions
            while oi < *mo {
                alignments.push(RowAlignment::Deleted(oi));
                oi += 1;
            }
            // all new rows before mn are additions
            while ni < *mn {
                alignments.push(RowAlignment::Added(ni));
                ni += 1;
            }
            // matched
            alignments.push(RowAlignment::Matched(*mo, *mn));
            oi = mo + 1;
            ni = mn + 1;
        }
        // tail deletions
        while oi < n {
            alignments.push(RowAlignment::Deleted(oi));
            oi += 1;
        }
        // tail additions
        while ni < m {
            alignments.push(RowAlignment::Added(ni));
            ni += 1;
        }
        return alignments;
    }

    // Fallback positional alignment if LCS is too expensive
    let mut alignments = Vec::new();
    let max_len = std::cmp::max(n, m);
    for i in 0..max_len {
        if i < n && i < m {
            alignments.push(RowAlignment::Matched(i, i));
        } else if i < n {
            alignments.push(RowAlignment::Deleted(i));
        } else {
            alignments.push(RowAlignment::Added(i));
        }
    }
    alignments
}

// 3. Compute Row/Cell Deltas in Parallel via Rayon with tolerance
fn compute_deltas_parallel(
    old_grid: &[Vec<Data>],
    new_grid: &[Vec<Data>],
    alignments: &[RowAlignment]
) -> Vec<CellDelta> {
    let eps = 1e-9_f64;
    alignments
        .par_iter()
        .flat_map(|alignment| {
            let mut deltas = Vec::new();
            match alignment {
                RowAlignment::Matched(old_idx, new_idx) => {
                    let old_row = &old_grid[*old_idx];
                    let new_row = &new_grid[*new_idx];
                    let max_cols = std::cmp::max(old_row.len(), new_row.len());

                    for c in 0..max_cols {
                        let old_cell = old_row.get(c).unwrap_or(&Data::Empty);
                        let new_cell = new_row.get(c).unwrap_or(&Data::Empty);

                        if !data_equal_with_tolerance(old_cell, new_cell, eps) {
                            deltas.push(CellDelta {
                                row_idx_old: Some(*old_idx),
                                row_idx_new: Some(*new_idx),
                                col_idx: c,
                                old_value: cell_to_string(old_cell),
                                new_value: cell_to_string(new_cell),
                                status: "Modified".to_string(),
                            });
                        }
                    }
                }
                RowAlignment::Deleted(old_idx) => {
                    let old_row = &old_grid[*old_idx];
                    for (c, cell) in old_row.iter().enumerate() {
                        if !matches!(cell, Data::Empty) {
                            deltas.push(CellDelta {
                                row_idx_old: Some(*old_idx),
                                row_idx_new: None,
                                col_idx: c,
                                old_value: cell_to_string(cell),
                                new_value: String::new(),
                                status: "Deleted".to_string(),
                            });
                        }
                    }
                }
                RowAlignment::Added(new_idx) => {
                    let new_row = &new_grid[*new_idx];
                    for (c, cell) in new_row.iter().enumerate() {
                        if !matches!(cell, Data::Empty) {
                            deltas.push(CellDelta {
                                row_idx_old: None,
                                row_idx_new: Some(*new_idx),
                                col_idx: c,
                                old_value: String::new(),
                                new_value: cell_to_string(cell),
                                status: "Added".to_string(),
                            });
                        }
                    }
                }
            }
            deltas
        })
        .collect()
}

// 4. Exposed Python Interface: GIL-free and panic-safe wrapper
#[pyfunction]
fn diff_sheets(py: Python,
    old_file: String,
    old_sheet: String,
    new_file: String,
    new_sheet: String,
    key_index: Option<usize>,
) -> PyResult<Vec<CellDelta>> {
    // Release the GIL and make the heavy work panic-safe
    py.allow_threads(|| {
        let res = catch_unwind(AssertUnwindSafe(|| {
            let old_grid = load_sheet_matrix(old_file, &old_sheet)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;
            let new_grid = load_sheet_matrix(new_file, &new_sheet)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;

            let alignments = align_matrices(&old_grid, &new_grid, key_index);
            let deltas = compute_deltas_parallel(&old_grid, &new_grid, &alignments);

            Ok(deltas)
        }));

        match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(pyerr)) => Err(pyerr),
            Err(_) => Err(PyRuntimeError::new_err("internal panic during diff computation")),
        }
    })
}

// Utility: list sheet names
#[pyfunction]
fn get_sheet_names(py: Python, file_path: String) -> PyResult<Vec<String>> {
    py.allow_threads(|| {
        list_sheet_names(file_path)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))
    })
}

#[pyfunction]
#[pyo3(signature = (old_file, new_file, key_index=None, selected_sheets=None))]
fn compare_workbooks_json(
    py: Python,
    old_file: String,
    new_file: String,
    key_index: Option<usize>,
    selected_sheets: Option<Vec<String>>,
) -> PyResult<String> {
    py.allow_threads(|| {
        let report = catch_unwind(AssertUnwindSafe(|| {
            compare_workbooks_report(&old_file, &new_file, key_index, selected_sheets)
        }));

        match report {
            Ok(Ok(value)) => serde_json::to_string(&value)
                .map_err(|e| PyRuntimeError::new_err(e.to_string())),
            Ok(Err(err)) => Err(pyo3::exceptions::PyValueError::new_err(err)),
            Err(_) => Err(PyRuntimeError::new_err("internal panic during workbook comparison")),
        }
    })
}

// Python module definition
#[pymodule]
fn xl_diff(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Initialize a global rayon threadpool with throttling to avoid saturating host
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(std::cmp::max(1, num_cpus::get().saturating_sub(1)))
        .build_global();

    m.add_function(wrap_pyfunction!(diff_sheets, m)?)?;
    m.add_function(wrap_pyfunction!(get_sheet_names, m)?)?;
    m.add_function(wrap_pyfunction!(compare_workbooks_json, m)?)?;
    m.add_class::<CellDelta>()?;
    Ok(())
}

// --- Unit tests for core logic (can run in CI) ---
#[cfg(test)]
mod tests {
    use super::*;
    use calamine::Data;

    #[test]
    fn test_trim_trailing_empty_rows() {
        let rows = vec![
            vec![Data::String("a".into())],
            vec![Data::Empty],
            vec![Data::Empty],
        ];
        let trimmed = trim_trailing_empty_rows(rows);
        assert_eq!(trimmed.len(), 1);
    }

    #[test]
    fn test_data_equality_tolerance() {
        let a = Data::Int(100);
        let b = Data::Float(100.0);
        assert!(data_equal_with_tolerance(&a, &b, 1e-9));

        let x = Data::Float(1.000000001);
        let y = Data::Float(1.000000002);
        assert!(data_equal_with_tolerance(&x, &y, 1e-8));
    }

    #[test]
    fn test_align_lcs_small() {
        let old = vec![
            vec![Data::String("k1".into()), Data::String("a".into())],
            vec![Data::String("k2".into()), Data::String("b".into())],
            vec![Data::String("kx".into()), Data::String("x".into())],
        ];
        let new = vec![
            vec![Data::String("k1".into()), Data::String("a".into())],
            vec![Data::String("k2".into()), Data::String("b".into())],
            vec![Data::String("k3".into()), Data::String("c".into())],
        ];
        let align = align_matrices(&old, &new, None);
        // Expect the stable rows to survive the LCS pass
        let matched = align.iter().filter(|a| matches!(a, RowAlignment::Matched(_, _))).count();
        assert!(matched >= 2);
    }

    #[test]
    fn test_sheet_name_selection_and_classification() {
        let old = vec!["Sheet1".to_string(), "Sheet2".to_string()];
        let new = vec!["Sheet2".to_string(), "Sheet3".to_string()];

        let old_set: HashSet<String> = old.iter().cloned().collect();
        let new_set: HashSet<String> = new.iter().cloned().collect();

        let deleted: Vec<_> = old.iter().filter(|s| !new_set.contains(*s)).cloned().collect();
        let added: Vec<_> = new.iter().filter(|s| !old_set.contains(*s)).cloned().collect();

        assert_eq!(deleted, vec!["Sheet1".to_string()]);
        assert_eq!(added, vec!["Sheet3".to_string()]);

        let selected = selected_sheet_filter(Some(vec!["Sheet2".to_string()]));
        assert!(should_compare_sheet("Sheet2", selected.as_ref()));
        assert!(!should_compare_sheet("Sheet1", selected.as_ref()));
    }
}