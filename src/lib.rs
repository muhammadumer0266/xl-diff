use pyo3::prelude::*;
use pyo3::wrap_pyfunction;
use calamine::{Reader, open_workbook_auto, Data};
use std::path::Path;
use std::collections::HashMap;
use rayon::prelude::*;

// Define the cell delta structure that will be exposed to Python
#[pyclass]
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

// 1. Load Excel Sheet Matrix
fn load_sheet_matrix<P: AsRef<Path>>(file_path: P, sheet_name: &str) -> Result<Vec<Vec<Data>>, String> {
    let mut workbook = open_workbook_auto(file_path).map_err(|e| e.to_string())?;

    // worksheet_range directly returns a Result, not an Option<Result>
    if let Ok(range) = workbook.worksheet_range(sheet_name) {
        // Explicitly annotating collect target eliminates E0282 inference errors
        Ok(range.rows().map(|row| row.to_vec()).collect::<Vec<Vec<Data>>>() )
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
    let mut alignments = Vec::new();

    if let Some(idx) = key_index {
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
    } else {
        let max_len = std::cmp::max(old_grid.len(), new_grid.len());
        for i in 0..max_len {
            if i < old_grid.len() && i < new_grid.len() {
                alignments.push(RowAlignment::Matched(i, i));
            } else if i < old_grid.len() {
                alignments.push(RowAlignment::Deleted(i));
            } else {
                alignments.push(RowAlignment::Added(i));
            }
        }
    }

    alignments
}

// 3. Compute Row/Cell Deltas in Parallel via Rayon
fn compute_deltas_parallel(
    old_grid: &[Vec<Data>], 
    new_grid: &[Vec<Data>], 
    alignments: &[RowAlignment]
) -> Vec<CellDelta> {
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

                        if old_cell != new_cell {
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
                        if *cell != Data::Empty {
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
                        if *cell != Data::Empty {
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

// 4. Exposed Python Interface
#[pyfunction]
fn diff_sheets(
    old_file: String,
    old_sheet: String,
    new_file: String,
    new_sheet: String,
    key_index: Option<usize>,
) -> PyResult<Vec<CellDelta>> {
    // Note: we intentionally keep this function synchronous at the Rust level. The Python caller
    // should call it inside a thread or use py.allow_threads when invoking from Python if needed.
    let old_grid = load_sheet_matrix(old_file, &old_sheet)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;
    let new_grid = load_sheet_matrix(new_file, &new_sheet)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;

    let alignments = align_matrices(&old_grid, &new_grid, key_index);
    let deltas = compute_deltas_parallel(&old_grid, &new_grid, &alignments);

    Ok(deltas)
}

// Utility: list sheet names
#[pyfunction]
fn get_sheet_names(file_path: String) -> PyResult<Vec<String>> {
    let workbook = open_workbook_auto(file_path)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok(workbook.sheet_names())
}

// Python module definition
#[pymodule]
fn xl_diff(py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(diff_sheets, m)?)?;
    m.add_function(wrap_pyfunction!(get_sheet_names, m)?)?;
    m.add_class::<CellDelta>()?;
    Ok(())
}

// --- Unit tests for core logic (can run in CI) ---
#[cfg(test)]
mod tests {
    use super::*;
    use calamine::Data;

    #[test]
    fn test_align_matrices_with_key() {
        let old = vec![
            vec![Data::String("k1".into()), Data::String("a".into())],
            vec![Data::String("k2".into()), Data::String("b".into())],
        ];
        let new = vec![
            vec![Data::String("k2".into()), Data::String("b_modified".into())],
            vec![Data::String("k1".into()), Data::String("a".into())],
            vec![Data::String("k3".into()), Data::String("c".into())],
        ];

        let align = align_matrices(&old, &new, Some(0));
        // Expect matches for k2 and k1 and an added k3, and no deletions
        assert!(align.iter().any(|a| matches!(a, RowAlignment::Matched(1,0))));
        assert!(align.iter().any(|a| matches!(a, RowAlignment::Matched(0,1))));
        assert!(align.iter().any(|a| matches!(a, RowAlignment::Added(2))));
    }

    #[test]
    fn test_compute_deltas_modified_added_deleted() {
        let old = vec![
            vec![Data::String("k1".into()), Data::String("a".into())],
            vec![Data::String("k2".into()), Data::String("b".into())],
        ];
        let new = vec![
            vec![Data::String("k2".into()), Data::String("b_modified".into())],
            vec![Data::String("k3".into()), Data::String("c".into())],
        ];

        let align = align_matrices(&old, &new, Some(0));
        let deltas = compute_deltas_parallel(&old, &new, &align);
        // Expect at least one modified (b -> b_modified), one deleted (k1), and one added (k3)
        assert!(deltas.iter().any(|d| d.status == "Modified"));
        assert!(deltas.iter().any(|d| d.status == "Deleted"));
        assert!(deltas.iter().any(|d| d.status == "Added"));
    }
}
