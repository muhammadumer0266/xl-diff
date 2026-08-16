// src/lib.rs
use pyo3::prelude::*;
use pyo3::wrap_pyfunction;
use pyo3::exceptions::PyRuntimeError;
use calamine::{Reader, open_workbook_auto, Data};
use std::path::Path;
use std::collections::{HashMap, HashSet};
use rayon::prelude::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::fs::{self, File};
use std::env;
use std::io::Read;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::Reader as XmlReader;
use zip::ZipArchive;
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
    #[pyo3(get)]
    pub value_changed: bool,
    #[pyo3(get)]
    pub formula_changed: bool,
    #[pyo3(get)]
    pub style_changed: bool,
    #[pyo3(get)]
    pub old_formula: Option<String>,
    #[pyo3(get)]
    pub new_formula: Option<String>,
    #[pyo3(get)]
    pub old_style_id: Option<u32>,
    #[pyo3(get)]
    pub new_style_id: Option<u32>,
    #[pyo3(get)]
    pub change_kinds: Vec<String>,
    #[pyo3(get)]
    pub formatting_changed: bool,
    #[pyo3(get)]
    pub formatting_changes: Vec<String>,
    #[pyo3(get)]
    pub old_style_profile: Option<CellStyleProfile>,
    #[pyo3(get)]
    pub new_style_profile: Option<CellStyleProfile>,
}

#[pyclass]
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CellStyleProfile {
    #[pyo3(get)]
    pub style_id: u32,
    #[pyo3(get)]
    pub number_format: String,
    #[pyo3(get)]
    pub font: String,
    #[pyo3(get)]
    pub fill: String,
    #[pyo3(get)]
    pub border: String,
    #[pyo3(get)]
    pub alignment: String,
    #[pyo3(get)]
    pub protection: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SheetDiffReport {
    pub sheet_name: String,
    pub status: String,
    pub renamed_from: Option<String>,
    pub renamed_to: Option<String>,
    pub row_count_old: Option<usize>,
    pub row_count_new: Option<usize>,
    pub value_changed_cells: usize,
    pub formula_changed_cells: usize,
    pub style_changed_cells: usize,
    pub cell_deltas: Vec<CellDelta>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkbookDiffSummary {
    pub total_sheets_old: usize,
    pub total_sheets_new: usize,
    pub compared_sheets: usize,
    pub added_sheets: usize,
    pub deleted_sheets: usize,
    pub renamed_sheets: usize,
    pub changed_sheets: usize,
    pub changed_cells: usize,
    pub value_changed_cells: usize,
    pub formula_changed_cells: usize,
    pub style_changed_cells: usize,
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

#[derive(Clone, Debug)]
struct StyleCatalog {
    styles: Vec<CellStyleProfile>,
}

fn format_xf_summary(
    style_id: u32,
    num_fmt_id: &str,
    num_fmt_code: Option<&String>,
    font_id: &str,
    fill_id: &str,
    border_id: &str,
    alignment: Option<&str>,
    protection: Option<&str>,
) -> CellStyleProfile {
    let number_format = match num_fmt_code {
        Some(code) if !code.is_empty() => format!("numFmtId={} code={}", num_fmt_id, code),
        _ => format!("numFmtId={}", num_fmt_id),
    };

    CellStyleProfile {
        style_id,
        number_format,
        font: format!("fontId={}", font_id),
        fill: format!("fillId={}", fill_id),
        border: format!("borderId={}", border_id),
        alignment: alignment.unwrap_or("alignment=default").to_string(),
        protection: protection.unwrap_or("protection=default").to_string(),
    }
}

fn compare_style_profiles(
    old_profile: Option<&CellStyleProfile>,
    new_profile: Option<&CellStyleProfile>,
) -> (bool, Vec<String>) {
    match (old_profile, new_profile) {
        (None, None) => (false, Vec::new()),
        (Some(old_style), Some(new_style)) => {
            let mut changes = Vec::new();
            if old_style.number_format != new_style.number_format {
                changes.push("number_format".to_string());
            }
            if old_style.font != new_style.font {
                changes.push("font".to_string());
            }
            if old_style.fill != new_style.fill {
                changes.push("fill".to_string());
            }
            if old_style.border != new_style.border {
                changes.push("border".to_string());
            }
            if old_style.alignment != new_style.alignment {
                changes.push("alignment".to_string());
            }
            if old_style.protection != new_style.protection {
                changes.push("protection".to_string());
            }
            (old_style != new_style, changes)
        }
        (Some(_), None) | (None, Some(_)) => (true, vec!["style_presence".to_string()]),
    }
}

fn load_style_catalog<P: AsRef<Path>>(file_path: P) -> Result<Option<StyleCatalog>, String> {
    let file_path_ref = file_path.as_ref();
    if !is_xlsx_like_path(file_path_ref) {
        return Ok(None);
    }

    let file = File::open(file_path_ref).map_err(|e| e.to_string())?;
    let mut zip = ZipArchive::new(file).map_err(|e| e.to_string())?;
    let styles_xml = match read_zip_entry_to_string(&mut zip, "xl/styles.xml") {
        Ok(xml) => xml,
        Err(_) => return Ok(None),
    };

    let mut reader = XmlReader::from_str(&styles_xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut num_fmt_codes: HashMap<String, String> = HashMap::new();
    let mut styles = Vec::new();
    let mut current_section = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) => match element.local_name().as_ref() {
                b"numFmts" => current_section = "numFmts".to_string(),
                b"cellXfs" => current_section = "cellXfs".to_string(),
                b"numFmt" if current_section == "numFmts" => {
                    let mut id = None;
                    let mut code = None;
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|e| e.to_string())?;
                        if attribute.key == QName(b"numFmtId") {
                            id = Some(attribute.decode_and_unescape_value(&reader).map_err(|e| e.to_string())?.into_owned());
                        } else if attribute.key == QName(b"formatCode") {
                            code = Some(attribute.decode_and_unescape_value(&reader).map_err(|e| e.to_string())?.into_owned());
                        }
                    }
                    if let (Some(id), Some(code)) = (id, code) {
                        num_fmt_codes.insert(id, code);
                    }
                }
                b"xf" if current_section == "cellXfs" => {
                    let mut num_fmt_id = String::from("0");
                    let mut font_id = String::from("0");
                    let mut fill_id = String::from("0");
                    let mut border_id = String::from("0");
                    let mut alignment_summary: Option<String> = None;
                    let mut protection_summary: Option<String> = None;

                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|e| e.to_string())?;
                        let value = attribute.decode_and_unescape_value(&reader).map_err(|e| e.to_string())?.into_owned();
                        if attribute.key == QName(b"numFmtId") {
                            num_fmt_id = value;
                        } else if attribute.key == QName(b"fontId") {
                            font_id = value;
                        } else if attribute.key == QName(b"fillId") {
                            fill_id = value;
                        } else if attribute.key == QName(b"borderId") {
                            border_id = value;
                        }
                    }

                    let mut inner_buf = Vec::new();
                    loop {
                        match reader.read_event_into(&mut inner_buf) {
                            Ok(Event::Start(inner)) if inner.local_name().as_ref() == b"alignment" => {
                                let mut parts = Vec::new();
                                for attribute in inner.attributes() {
                                    let attribute = attribute.map_err(|e| e.to_string())?;
                                    let value = attribute.decode_and_unescape_value(&reader).map_err(|e| e.to_string())?.into_owned();
                                    parts.push(format!("{}={}", String::from_utf8_lossy(attribute.key.as_ref()), value));
                                }
                                if !parts.is_empty() {
                                    alignment_summary = Some(parts.join(";"));
                                }
                            }
                            Ok(Event::Empty(inner)) if inner.local_name().as_ref() == b"alignment" => {
                                let mut parts = Vec::new();
                                for attribute in inner.attributes() {
                                    let attribute = attribute.map_err(|e| e.to_string())?;
                                    let value = attribute.decode_and_unescape_value(&reader).map_err(|e| e.to_string())?.into_owned();
                                    parts.push(format!("{}={}", String::from_utf8_lossy(attribute.key.as_ref()), value));
                                }
                                if !parts.is_empty() {
                                    alignment_summary = Some(parts.join(";"));
                                }
                            }
                            Ok(Event::Start(inner)) if inner.local_name().as_ref() == b"protection" => {
                                let mut parts = Vec::new();
                                for attribute in inner.attributes() {
                                    let attribute = attribute.map_err(|e| e.to_string())?;
                                    let value = attribute.decode_and_unescape_value(&reader).map_err(|e| e.to_string())?.into_owned();
                                    parts.push(format!("{}={}", String::from_utf8_lossy(attribute.key.as_ref()), value));
                                }
                                if !parts.is_empty() {
                                    protection_summary = Some(parts.join(";"));
                                }
                            }
                            Ok(Event::Empty(inner)) if inner.local_name().as_ref() == b"protection" => {
                                let mut parts = Vec::new();
                                for attribute in inner.attributes() {
                                    let attribute = attribute.map_err(|e| e.to_string())?;
                                    let value = attribute.decode_and_unescape_value(&reader).map_err(|e| e.to_string())?.into_owned();
                                    parts.push(format!("{}={}", String::from_utf8_lossy(attribute.key.as_ref()), value));
                                }
                                if !parts.is_empty() {
                                    protection_summary = Some(parts.join(";"));
                                }
                            }
                            Ok(Event::End(inner)) if inner.local_name().as_ref() == b"xf" => break,
                            Ok(Event::Eof) => break,
                            Err(err) => return Err(err.to_string()),
                            _ => {}
                        }
                        inner_buf.clear();
                    }

                    let style_id = styles.len() as u32;
                    let profile = format_xf_summary(
                        style_id,
                        &num_fmt_id,
                        num_fmt_codes.get(&num_fmt_id),
                        &font_id,
                        &fill_id,
                        &border_id,
                        alignment_summary.as_deref(),
                        protection_summary.as_deref(),
                    );
                    styles.push(profile);
                }
                _ => {}
            },
            Ok(Event::End(element)) => match element.local_name().as_ref() {
                b"numFmts" | b"cellXfs" => current_section.clear(),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(err) => return Err(err.to_string()),
            _ => {}
        }
        buf.clear();
    }

    if styles.is_empty() {
        return Ok(None);
    }

    Ok(Some(StyleCatalog { styles }))
}

fn resolve_style_profile(catalog: Option<&StyleCatalog>, style_id: Option<u32>) -> Option<CellStyleProfile> {
    let style_id = style_id?;
    catalog.and_then(|catalog| catalog.styles.get(style_id as usize).cloned()).or_else(|| {
        Some(CellStyleProfile {
            style_id,
            number_format: format!("numFmtId={}", style_id),
            font: "fontId=unknown".to_string(),
            fill: "fillId=unknown".to_string(),
            border: "borderId=unknown".to_string(),
            alignment: "alignment=unknown".to_string(),
            protection: "protection=unknown".to_string(),
        })
    })
}

#[derive(Clone, Debug)]
struct SheetSnapshot {
    values: Vec<Vec<Data>>,
    formulas: Vec<Vec<String>>,
    style_ids: Option<Vec<Vec<Option<u32>>>>,
}

fn is_xlsx_like_path<P: AsRef<Path>>(file_path: P) -> bool {
    match file_path.as_ref().extension().and_then(|ext| ext.to_str()) {
        Some(ext) => matches!(ext.to_ascii_lowercase().as_str(), "xlsx" | "xlsm" | "xlam"),
        None => false,
    }
}

fn read_zip_entry_to_string(zip: &mut ZipArchive<File>, entry_name: &str) -> Result<String, String> {
    let mut file = zip.by_name(entry_name).map_err(|e| e.to_string())?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(|e| e.to_string())?;
    Ok(contents)
}

fn attr_value(reader: &XmlReader<&[u8]>, element: &BytesStart<'_>, key: &[u8]) -> Result<Option<String>, String> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|e| e.to_string())?;
        if attribute.key == QName(key) {
            return Ok(Some(
                attribute
                    .decode_and_unescape_value(reader)
                    .map_err(|e| e.to_string())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn excel_column_to_index(column: &str) -> Option<usize> {
    let mut value = 0usize;
    if column.is_empty() {
        return None;
    }
    for ch in column.chars() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        value = value * 26 + ((ch.to_ascii_uppercase() as usize) - ('A' as usize) + 1);
    }
    Some(value.saturating_sub(1))
}

fn excel_cell_ref_to_indices(cell_ref: &str) -> Option<(usize, usize)> {
    let mut column = String::new();
    let mut row = String::new();
    for ch in cell_ref.chars() {
        if ch.is_ascii_alphabetic() {
            column.push(ch);
        } else if ch.is_ascii_digit() {
            row.push(ch);
        }
    }
    let row_index = row.parse::<usize>().ok()?.saturating_sub(1);
    let col_index = excel_column_to_index(&column)?;
    Some((row_index, col_index))
}

fn ensure_matrix_slot(matrix: &mut Vec<Vec<Option<u32>>>, row: usize, col: usize) {
    if matrix.len() <= row {
        matrix.resize_with(row + 1, Vec::new);
    }
    if matrix[row].len() <= col {
        matrix[row].resize(col + 1, None);
    }
}

fn trim_trailing_none_rows(mut rows: Vec<Vec<Option<u32>>>) -> Vec<Vec<Option<u32>>> {
    while let Some(last) = rows.last() {
        if last.iter().all(|item| item.is_none()) {
            rows.pop();
        } else {
            break;
        }
    }
    rows
}

fn load_sheet_formula_matrix<P: AsRef<Path>>(file_path: P, sheet_name: &str) -> Result<Vec<Vec<String>>, String> {
    let mut workbook = open_workbook_auto(file_path).map_err(|e| e.to_string())?;
    let range = workbook.worksheet_formula(sheet_name).map_err(|e| e.to_string())?;
    Ok(range
        .rows()
        .map(|row| row.iter().map(|cell| cell.to_string()).collect())
        .collect())
}

fn xlsx_sheet_targets(file_path: &str) -> Result<HashMap<String, String>, String> {
    let file = File::open(file_path).map_err(|e| e.to_string())?;
    let mut zip = ZipArchive::new(file).map_err(|e| e.to_string())?;
    let workbook_xml = read_zip_entry_to_string(&mut zip, "xl/workbook.xml")?;
    let rels_xml = read_zip_entry_to_string(&mut zip, "xl/_rels/workbook.xml.rels")?;

    let mut rel_map: HashMap<String, String> = HashMap::new();
    let mut reader = XmlReader::from_str(&rels_xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(element)) | Ok(Event::Start(element))
                if element.local_name().as_ref() == b"Relationship" => {
                let mut id = None;
                let mut target = None;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|e| e.to_string())?;
                    if attribute.key == QName(b"Id") {
                        id = Some(
                            attribute
                                .decode_and_unescape_value(&reader)
                                .map_err(|e| e.to_string())?
                                .into_owned(),
                        );
                    } else if attribute.key == QName(b"Target") {
                        target = Some(
                            attribute
                                .decode_and_unescape_value(&reader)
                                .map_err(|e| e.to_string())?
                                .into_owned(),
                        );
                    }
                }
                if let (Some(id), Some(target)) = (id, target) {
                    rel_map.insert(id, target);
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(err.to_string()),
            _ => {}
        }
        buf.clear();
    }

    let mut sheet_targets = HashMap::new();
    let mut reader = XmlReader::from_str(&workbook_xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(element)) | Ok(Event::Start(element))
                if element.local_name().as_ref() == b"sheet" => {
                let mut name = None;
                let mut rel_id = None;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|e| e.to_string())?;
                    if attribute.key == QName(b"name") {
                        name = Some(
                            attribute
                                .decode_and_unescape_value(&reader)
                                .map_err(|e| e.to_string())?
                                .into_owned(),
                        );
                    } else if attribute.key == QName(b"r:id") {
                        rel_id = Some(
                            attribute
                                .decode_and_unescape_value(&reader)
                                .map_err(|e| e.to_string())?
                                .into_owned(),
                        );
                    }
                }
                if let (Some(name), Some(rel_id)) = (name, rel_id) {
                    if let Some(target) = rel_map.get(&rel_id) {
                        let normalized = if target.starts_with("xl/") {
                            target.clone()
                        } else {
                            format!("xl/{}", target.trim_start_matches('/'))
                        };
                        sheet_targets.insert(name, normalized);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(err.to_string()),
            _ => {}
        }
        buf.clear();
    }

    Ok(sheet_targets)
}

fn load_sheet_style_matrix<P: AsRef<Path>>(file_path: P, sheet_name: &str) -> Result<Option<Vec<Vec<Option<u32>>>>, String> {
    let file_path_ref = file_path.as_ref();
    if !is_xlsx_like_path(file_path_ref) {
        return Ok(None);
    }

    let sheet_targets = xlsx_sheet_targets(file_path_ref.to_string_lossy().as_ref())?;
    let sheet_path = match sheet_targets.get(sheet_name) {
        Some(path) => path.clone(),
        None => return Err(format!("Sheet '{}' not found or failed to resolve path.", sheet_name)),
    };

    let file = File::open(file_path_ref).map_err(|e| e.to_string())?;
    let mut zip = ZipArchive::new(file).map_err(|e| e.to_string())?;
    let sheet_xml = read_zip_entry_to_string(&mut zip, &sheet_path)?;

    let mut reader = XmlReader::from_str(&sheet_xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut inner_buf = Vec::new();
    let mut matrix: Vec<Vec<Option<u32>>> = Vec::new();
    let mut row_index = 0usize;
    let mut col_index = 0usize;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) if element.local_name().as_ref() == b"row" => {
                if let Some(row_ref) = attr_value(&reader, &element, b"r")? {
                    if let Ok(row) = row_ref.parse::<usize>() {
                        row_index = row.saturating_sub(1);
                    }
                }
                if matrix.len() <= row_index {
                    matrix.resize_with(row_index + 1, Vec::new);
                }
                col_index = 0;
            }
            Ok(Event::End(element)) if element.local_name().as_ref() == b"row" => {
                row_index = row_index.saturating_add(1);
                col_index = 0;
            }
            Ok(Event::Empty(element)) | Ok(Event::Start(element)) if element.local_name().as_ref() == b"c" => {
                let (parsed_row, parsed_col) = match attr_value(&reader, &element, b"r")? {
                    Some(cell_ref) => excel_cell_ref_to_indices(&cell_ref).unwrap_or((row_index, col_index)),
                    None => (row_index, col_index),
                };
                row_index = parsed_row;
                col_index = parsed_col;

                let style_id = match attr_value(&reader, &element, b"s")? {
                    Some(style) => style.parse::<u32>().ok(),
                    None => None,
                };

                ensure_matrix_slot(&mut matrix, row_index, col_index);
                matrix[row_index][col_index] = style_id;

                if matches!(reader.read_event_into(&mut inner_buf), Ok(Event::End(end)) if end.local_name().as_ref() == b"c") {
                } else {
                    loop {
                        inner_buf.clear();
                        match reader.read_event_into(&mut inner_buf) {
                            Ok(Event::End(end)) if end.local_name().as_ref() == b"c" => break,
                            Ok(Event::Eof) => break,
                            Err(err) => return Err(err.to_string()),
                            _ => {}
                        }
                    }
                }
                col_index = col_index.saturating_add(1);
            }
            Ok(Event::Empty(element)) if element.local_name().as_ref() == b"mergeCell" => {
                let _ = attr_value(&reader, &element, b"ref")?;
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(err.to_string()),
            _ => {}
        }
        buf.clear();
    }

    Ok(Some(trim_trailing_none_rows(matrix)))
}

fn load_sheet_snapshot<P: AsRef<Path>>(file_path: P, sheet_name: &str) -> Result<SheetSnapshot, String> {
    let values = load_sheet_matrix(&file_path, sheet_name)?;
    let formulas = load_sheet_formula_matrix(&file_path, sheet_name)?;
    let style_ids = load_sheet_style_matrix(&file_path, sheet_name)?;
    Ok(SheetSnapshot { values, formulas, style_ids })
}

fn resolve_style_matrix(
    catalog: Option<&StyleCatalog>,
    style_ids: Option<&[Vec<Option<u32>>]>,
) -> Option<Vec<Vec<Option<CellStyleProfile>>>> {
    let style_ids = style_ids?;
    Some(
        style_ids
            .iter()
            .map(|row| {
                row.iter()
                    .map(|style_id| resolve_style_profile(catalog, *style_id))
                    .collect()
            })
            .collect(),
    )
}

fn combine_row_signatures(
    row_idx: usize,
    values: &[Vec<Data>],
    formulas: &[Vec<String>],
    styles: Option<&[Vec<Option<u32>>]>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    let value_row = values.get(row_idx);
    let formula_row = formulas.get(row_idx);
    let style_row = styles.and_then(|matrix| matrix.get(row_idx));
    let max_cols = value_row
        .map(|row| row.len())
        .unwrap_or(0)
        .max(formula_row.map(|row| row.len()).unwrap_or(0))
        .max(style_row.map(|row| row.len()).unwrap_or(0));

    for col_idx in 0..max_cols {
        let value = value_row.and_then(|row| row.get(col_idx)).cloned().unwrap_or(Data::Empty);
        let formula = formula_row.and_then(|row| row.get(col_idx)).cloned().unwrap_or_default();
        let style = style_row.and_then(|row| row.get(col_idx)).and_then(|id| *id);

        hasher.update(cell_to_string(&value).as_bytes());
        hasher.update(&[0x1f]);
        hasher.update(formula.as_bytes());
        hasher.update(&[0x1f]);
        hasher.update(style.map(|id| id.to_string()).unwrap_or_default().as_bytes());
        hasher.update(&[0x1f]);
    }

    let digest = hasher.finalize();
    hex::encode(digest.as_bytes())
}

fn sheet_signature(snapshot: &SheetSnapshot) -> String {
    let row_count = snapshot.values.len().max(snapshot.formulas.len()).max(
        snapshot
            .style_ids
            .as_ref()
            .map(|matrix| matrix.len())
            .unwrap_or(0),
    );
    let mut hasher = blake3::Hasher::new();
    for row_idx in 0..row_count {
        let row_signature = combine_row_signatures(
            row_idx,
            &snapshot.values,
            &snapshot.formulas,
            snapshot.style_ids.as_deref(),
        );
        hasher.update(row_signature.as_bytes());
        hasher.update(&[0x1f]);
    }
    let digest = hasher.finalize();
    hex::encode(digest.as_bytes())
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
    let selected_sheet_list = selected_sheets.clone().unwrap_or_default();
    let selected_lookup = selected_sheet_filter(selected_sheets);
    let old_style_catalog = load_style_catalog(old_file)?;
    let new_style_catalog = load_style_catalog(new_file)?;

    let old_set: HashSet<String> = old_sheet_names.iter().cloned().collect();
    let new_set: HashSet<String> = new_sheet_names.iter().cloned().collect();

    let mut reports = Vec::new();
    let mut deleted_snapshots: Vec<(String, SheetSnapshot, String)> = Vec::new();
    let mut added_snapshots: Vec<(String, SheetSnapshot, String)> = Vec::new();
    let mut compared_sheets = 0usize;
    let mut changed_sheets = 0usize;
    let mut changed_cells = 0usize;
    let mut value_changed_cells = 0usize;
    let mut formula_changed_cells = 0usize;
    let mut style_changed_cells = 0usize;

    for sheet_name in &old_sheet_names {
        if !new_set.contains(sheet_name) {
            let snapshot = load_sheet_snapshot(old_file, sheet_name)?;
            let signature = sheet_signature(&snapshot);
            deleted_snapshots.push((sheet_name.clone(), snapshot, signature));
        }
    }

    for sheet_name in &new_sheet_names {
        if !old_set.contains(sheet_name) {
            let snapshot = load_sheet_snapshot(new_file, sheet_name)?;
            let signature = sheet_signature(&snapshot);
            added_snapshots.push((sheet_name.clone(), snapshot, signature));
        }
    }

    for sheet_name in &old_sheet_names {
        if new_set.contains(sheet_name) && should_compare_sheet(sheet_name, selected_lookup.as_ref()) {
            let old_snapshot = load_sheet_snapshot(old_file, sheet_name)?;
            let new_snapshot = load_sheet_snapshot(new_file, sheet_name)?;
            let old_style_profiles = resolve_style_matrix(old_style_catalog.as_ref(), old_snapshot.style_ids.as_deref());
            let new_style_profiles = resolve_style_matrix(new_style_catalog.as_ref(), new_snapshot.style_ids.as_deref());
            let alignments = align_matrices(&old_snapshot.values, &new_snapshot.values, key_index);
            let cell_deltas = compute_deltas_parallel(
                &old_snapshot.values,
                &new_snapshot.values,
                Some(&old_snapshot.formulas),
                Some(&new_snapshot.formulas),
                old_style_profiles.as_deref(),
                new_style_profiles.as_deref(),
                &alignments,
            );

            let sheet_value_changed_cells = cell_deltas.iter().filter(|delta| delta.value_changed).count();
            let sheet_formula_changed_cells = cell_deltas.iter().filter(|delta| delta.formula_changed).count();
            let sheet_style_changed_cells = cell_deltas.iter().filter(|delta| delta.style_changed).count();

            if !cell_deltas.is_empty() {
                changed_sheets += 1;
                changed_cells += cell_deltas.len();
                value_changed_cells += sheet_value_changed_cells;
                formula_changed_cells += sheet_formula_changed_cells;
                style_changed_cells += sheet_style_changed_cells;
            }
            compared_sheets += 1;
            reports.push(SheetDiffReport {
                sheet_name: sheet_name.clone(),
                status: if cell_deltas.is_empty() {
                    "Unchanged".to_string()
                } else {
                    "Compared".to_string()
                },
                renamed_from: None,
                renamed_to: None,
                row_count_old: Some(old_snapshot.values.len()),
                row_count_new: Some(new_snapshot.values.len()),
                value_changed_cells: sheet_value_changed_cells,
                formula_changed_cells: sheet_formula_changed_cells,
                style_changed_cells: sheet_style_changed_cells,
                cell_deltas,
            });
        }
    }

    let mut deleted_by_signature: HashMap<String, Vec<(String, SheetSnapshot)>> = HashMap::new();
    for (sheet_name, snapshot, signature) in deleted_snapshots {
        deleted_by_signature.entry(signature).or_default().push((sheet_name, snapshot));
    }

    let mut added_by_signature: HashMap<String, Vec<(String, SheetSnapshot)>> = HashMap::new();
    for (sheet_name, snapshot, signature) in added_snapshots {
        added_by_signature.entry(signature).or_default().push((sheet_name, snapshot));
    }

    let mut renamed_reports = Vec::new();
    let mut leftover_deleted = Vec::new();
    let mut leftover_added = Vec::new();
    let signatures: HashSet<String> = deleted_by_signature.keys().chain(added_by_signature.keys()).cloned().collect();

    for signature in signatures {
        let deleted_items = deleted_by_signature.remove(&signature).unwrap_or_default();
        let added_items = added_by_signature.remove(&signature).unwrap_or_default();
        let rename_count = std::cmp::min(deleted_items.len(), added_items.len());

        for index in 0..rename_count {
            let (deleted_name, deleted_snapshot) = &deleted_items[index];
            let (added_name, added_snapshot) = &added_items[index];
            renamed_reports.push(SheetDiffReport {
                sheet_name: added_name.clone(),
                status: "Renamed".to_string(),
                renamed_from: Some(deleted_name.clone()),
                renamed_to: Some(added_name.clone()),
                row_count_old: Some(deleted_snapshot.values.len()),
                row_count_new: Some(added_snapshot.values.len()),
                value_changed_cells: 0,
                formula_changed_cells: 0,
                style_changed_cells: 0,
                cell_deltas: Vec::new(),
            });
        }

        for item in deleted_items.into_iter().skip(rename_count) {
            leftover_deleted.push(item);
        }

        for item in added_items.into_iter().skip(rename_count) {
            leftover_added.push(item);
        }
    }

    for (sheet_name, snapshot) in leftover_deleted {
        reports.push(SheetDiffReport {
            sheet_name,
            status: "Deleted".to_string(),
            renamed_from: None,
            renamed_to: None,
            row_count_old: Some(snapshot.values.len()),
            row_count_new: None,
            value_changed_cells: 0,
            formula_changed_cells: 0,
            style_changed_cells: 0,
            cell_deltas: Vec::new(),
        });
    }

    for (sheet_name, snapshot) in leftover_added {
        reports.push(SheetDiffReport {
            sheet_name,
            status: "Added".to_string(),
            renamed_from: None,
            renamed_to: None,
            row_count_old: None,
            row_count_new: Some(snapshot.values.len()),
            value_changed_cells: 0,
            formula_changed_cells: 0,
            style_changed_cells: 0,
            cell_deltas: Vec::new(),
        });
    }

    reports.splice(0..0, renamed_reports);

    Ok(WorkbookDiffReport {
        old_file: old_file.to_string(),
        new_file: new_file.to_string(),
        selected_sheets: selected_sheet_list,
        summary: WorkbookDiffSummary {
            total_sheets_old: old_sheet_names.len(),
            total_sheets_new: new_sheet_names.len(),
            compared_sheets,
            added_sheets: reports.iter().filter(|report| report.status == "Added").count(),
            deleted_sheets: reports.iter().filter(|report| report.status == "Deleted").count(),
            renamed_sheets: reports.iter().filter(|report| report.status == "Renamed").count(),
            changed_sheets,
            changed_cells,
            value_changed_cells,
            formula_changed_cells,
            style_changed_cells,
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
    old_formula_grid: Option<&[Vec<String>]>,
    new_formula_grid: Option<&[Vec<String>]>,
    old_style_grid: Option<&[Vec<Option<CellStyleProfile>>]>,
    new_style_grid: Option<&[Vec<Option<CellStyleProfile>>]>,
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
                    let old_formula_row = old_formula_grid.and_then(|grid| grid.get(*old_idx));
                    let new_formula_row = new_formula_grid.and_then(|grid| grid.get(*new_idx));
                    let old_style_row = old_style_grid.and_then(|grid| grid.get(*old_idx));
                    let new_style_row = new_style_grid.and_then(|grid| grid.get(*new_idx));
                    let max_cols = std::cmp::max(
                        std::cmp::max(old_row.len(), new_row.len()),
                        std::cmp::max(
                            old_formula_row.map(|row| row.len()).unwrap_or(0),
                            new_formula_row.map(|row| row.len()).unwrap_or(0),
                        ),
                    )
                    .max(
                        std::cmp::max(
                            old_style_row.map(|row| row.len()).unwrap_or(0),
                            new_style_row.map(|row| row.len()).unwrap_or(0),
                        ),
                    );

                    for c in 0..max_cols {
                        let old_cell = old_row.get(c).unwrap_or(&Data::Empty);
                        let new_cell = new_row.get(c).unwrap_or(&Data::Empty);
                        let old_formula = old_formula_row
                            .and_then(|row| row.get(c))
                            .cloned()
                            .unwrap_or_default();
                        let new_formula = new_formula_row
                            .and_then(|row| row.get(c))
                            .cloned()
                            .unwrap_or_default();
                        let old_style_profile = old_style_row
                            .and_then(|row| row.get(c))
                            .copied()
                            .flatten();
                        let new_style_profile = new_style_row
                            .and_then(|row| row.get(c))
                            .copied()
                            .flatten();
                        let old_style_id = old_style_profile.as_ref().map(|profile| profile.style_id);
                        let new_style_id = new_style_profile.as_ref().map(|profile| profile.style_id);

                        let value_changed = !data_equal_with_tolerance(old_cell, new_cell, eps);
                        let formula_changed = old_formula != new_formula;
                        let (style_changed, mut formatting_changes) = compare_style_profiles(
                            old_style_profile.as_ref(),
                            new_style_profile.as_ref(),
                        );

                        if value_changed || formula_changed || style_changed {
                            let mut change_kinds = Vec::new();
                            if value_changed {
                                change_kinds.push("Value".to_string());
                            }
                            if formula_changed {
                                change_kinds.push("Formula".to_string());
                            }
                            if style_changed {
                                change_kinds.push("Style".to_string());
                            }
                            if !formatting_changes.is_empty() {
                                change_kinds.push("Formatting".to_string());
                            }

                            let status = if formula_changed && !value_changed && !style_changed {
                                "FormulaModified"
                            } else if style_changed && !value_changed && !formula_changed {
                                "StyleModified"
                            } else if value_changed && !formula_changed && !style_changed {
                                "ValueModified"
                            } else {
                                "Modified"
                            };

                            deltas.push(CellDelta {
                                row_idx_old: Some(*old_idx),
                                row_idx_new: Some(*new_idx),
                                col_idx: c,
                                old_value: cell_to_string(old_cell),
                                new_value: cell_to_string(new_cell),
                                status: status.to_string(),
                                value_changed,
                                formula_changed,
                                style_changed,
                                old_formula: if old_formula.is_empty() { None } else { Some(old_formula) },
                                new_formula: if new_formula.is_empty() { None } else { Some(new_formula) },
                                old_style_id,
                                new_style_id,
                                change_kinds,
                                formatting_changed: style_changed,
                                formatting_changes,
                                old_style_profile,
                                new_style_profile,
                            });
                        }
                    }
                }
                RowAlignment::Deleted(old_idx) => {
                    let old_row = &old_grid[*old_idx];
                    let old_formula_row = old_formula_grid.and_then(|grid| grid.get(*old_idx));
                    let old_style_row = old_style_grid.and_then(|grid| grid.get(*old_idx));
                    for (c, cell) in old_row.iter().enumerate() {
                        if !matches!(cell, Data::Empty) {
                            let old_formula = old_formula_row
                                .and_then(|row| row.get(c))
                                .cloned()
                                .unwrap_or_default();
                            let old_style_id = old_style_row
                                .and_then(|row| row.get(c))
                                .copied()
                                .flatten();
                            let mut change_kinds = vec!["Deleted".to_string()];
                            if !old_formula.is_empty() {
                                change_kinds.push("Formula".to_string());
                            }
                            if old_style_id.is_some() {
                                change_kinds.push("Style".to_string());
                            }
                            deltas.push(CellDelta {
                                row_idx_old: Some(*old_idx),
                                row_idx_new: None,
                                col_idx: c,
                                old_value: cell_to_string(cell),
                                new_value: String::new(),
                                status: "Deleted".to_string(),
                                value_changed: true,
                                formula_changed: !old_formula.is_empty(),
                                style_changed: old_style_id.is_some(),
                                old_formula: if old_formula.is_empty() { None } else { Some(old_formula) },
                                new_formula: None,
                                old_style_id,
                                new_style_id: None,
                                change_kinds,
                                formatting_changed: old_style_id.is_some(),
                                formatting_changes: vec!["style_removed".to_string()],
                                old_style_profile,
                                new_style_profile: None,
                            });
                        }
                    }
                }
                RowAlignment::Added(new_idx) => {
                    let new_row = &new_grid[*new_idx];
                    let new_formula_row = new_formula_grid.and_then(|grid| grid.get(*new_idx));
                    let new_style_row = new_style_grid.and_then(|grid| grid.get(*new_idx));
                    for (c, cell) in new_row.iter().enumerate() {
                        if !matches!(cell, Data::Empty) {
                            let new_formula = new_formula_row
                                .and_then(|row| row.get(c))
                                .cloned()
                                .unwrap_or_default();
                            let new_style_id = new_style_row
                                .and_then(|row| row.get(c))
                                .copied()
                                .flatten();
                            let mut change_kinds = vec!["Added".to_string()];
                            if !new_formula.is_empty() {
                                change_kinds.push("Formula".to_string());
                            }
                            if new_style_id.is_some() {
                                change_kinds.push("Style".to_string());
                            }
                            deltas.push(CellDelta {
                                row_idx_old: None,
                                row_idx_new: Some(*new_idx),
                                col_idx: c,
                                old_value: String::new(),
                                new_value: cell_to_string(cell),
                                status: "Added".to_string(),
                                value_changed: true,
                                formula_changed: !new_formula.is_empty(),
                                style_changed: new_style_id.is_some(),
                                old_formula: None,
                                new_formula: if new_formula.is_empty() { None } else { Some(new_formula) },
                                old_style_id: None,
                                new_style_id,
                                change_kinds,
                                formatting_changed: new_style_id.is_some(),
                                formatting_changes: vec!["style_added".to_string()],
                                old_style_profile: None,
                                new_style_profile,
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
            let old_snapshot = load_sheet_snapshot(&old_file, &old_sheet)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;
            let new_snapshot = load_sheet_snapshot(&new_file, &new_sheet)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;
            let old_style_catalog = load_style_catalog(&old_file)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;
            let new_style_catalog = load_style_catalog(&new_file)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e))?;
            let old_style_profiles = resolve_style_matrix(old_style_catalog.as_ref(), old_snapshot.style_ids.as_deref());
            let new_style_profiles = resolve_style_matrix(new_style_catalog.as_ref(), new_snapshot.style_ids.as_deref());

            let alignments = align_matrices(&old_snapshot.values, &new_snapshot.values, key_index);
            let deltas = compute_deltas_parallel(
                &old_snapshot.values,
                &new_snapshot.values,
                Some(&old_snapshot.formulas),
                Some(&new_snapshot.formulas),
                old_style_profiles.as_deref(),
                new_style_profiles.as_deref(),
                &alignments,
            );

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