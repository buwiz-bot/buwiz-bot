//! The spreadsheet: `.xlsx`/`.xlsm`, `.xls`, `.csv` and `.tsv`.

#[cfg(test)]
mod bench;
pub(crate) mod convert;
pub(crate) mod doc;
pub(crate) mod select;

/// A path's extension, lowercased.
pub(crate) fn ext(path: &std::path::Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}
