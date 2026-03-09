//! TUI Application state.

use std::path::PathBuf;

use ratatui::widgets::ListState;
use uffs_core::MftQuery;
use uffs_mft::MftReader;
use uffs_polars::DataFrame;

/// A search result item.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// File or directory name.
    pub name: String,
    /// File size in bytes.
    pub size: u64,
    /// File Record Segment number (used for detail views).
    #[expect(dead_code, reason = "populated for future detail view feature")]
    pub frs: u64,
    /// Whether this is a directory.
    pub is_directory: bool,
}

/// Application state.
#[expect(
    clippy::partial_pub_fields,
    reason = "dataframe is intentionally private; accessed only through methods"
)]
pub struct App {
    /// Current search input.
    pub input: String,
    /// Search results.
    pub results: Vec<SearchResult>,
    /// List selection state.
    pub list_state: ListState,
    /// Loaded `DataFrame` (if any).
    dataframe: Option<DataFrame>,
    /// Path to loaded index file.
    pub index_path: Option<PathBuf>,
    /// Status message.
    pub status: String,
    /// Error message (if any).
    pub error: Option<String>,
}

impl App {
    /// Create a new application.
    pub fn new() -> Self {
        Self {
            input: String::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            dataframe: None,
            index_path: None,
            status: "No index loaded. Press 'l' to load an index file.".to_owned(),
            error: None,
        }
    }

    /// Create application with a pre-loaded DataFrame.
    pub fn with_dataframe(df: DataFrame, path: Option<PathBuf>) -> Self {
        let record_count = df.height();
        Self {
            input: String::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            dataframe: Some(df),
            index_path: path.clone(),
            status: format!(
                "Loaded {} records{}",
                record_count,
                path.map_or(String::new(), |path| format!(" from {}", path.display()))
            ),
            error: None,
        }
    }

    /// Load an index from a Parquet file.
    #[expect(
        dead_code,
        reason = "public API for runtime index loading; not yet wired to a keybinding"
    )]
    pub fn load_index(&mut self, path: &std::path::Path) -> Result<(), String> {
        match MftReader::load_parquet(path) {
            Ok(df) => {
                let count = df.height();
                self.dataframe = Some(df);
                self.index_path = Some(path.to_path_buf());
                self.status = format!("Loaded {} records from {}", count, path.display());
                self.error = None;
                Ok(())
            }
            Err(err) => {
                self.error = Some(format!("Failed to load index: {err}"));
                Err(format!("Failed to load index: {err}"))
            }
        }
    }

    /// Check if an index is loaded.
    #[must_use]
    pub const fn has_index(&self) -> bool {
        self.dataframe.is_some()
    }

    /// Move selection to next item.
    pub fn next(&mut self) {
        let idx = match self.list_state.selected() {
            Some(current) => {
                if current >= self.results.len().saturating_sub(1) {
                    0
                } else {
                    current + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(idx));
    }

    /// Move selection to previous item.
    pub fn previous(&mut self) {
        let idx = match self.list_state.selected() {
            Some(current) => {
                if current == 0 {
                    self.results.len().saturating_sub(1)
                } else {
                    current - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(idx));
    }

    /// Execute search with current input.
    pub fn search(&mut self) {
        self.error = None;

        if self.input.is_empty() {
            self.results.clear();
            return;
        }

        let Some(df) = &self.dataframe else {
            self.error = Some("No index loaded. Press 'l' to load an index file.".to_owned());
            return;
        };

        // Build and execute query
        let query = MftQuery::new(df.clone());
        let pattern = if self.input.contains('*') || self.input.contains('?') {
            self.input.clone()
        } else {
            // If no glob chars, wrap in wildcards for substring match
            format!("*{}*", self.input)
        };

        match query.glob(&pattern) {
            Ok(filtered) => match filtered.limit(100).collect() {
                Ok(result_df) => {
                    self.results = dataframe_to_results(&result_df);
                    self.status = format!("Found {} results", self.results.len());
                    if !self.results.is_empty() {
                        self.list_state.select(Some(0));
                    }
                }
                Err(err) => {
                    self.error = Some(format!("Query failed: {err}"));
                }
            },
            Err(err) => {
                self.error = Some(format!("Invalid pattern: {err}"));
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a `DataFrame` to a vector of `SearchResult`.
#[expect(
    clippy::single_call_fn,
    reason = "separated from search method for readability"
)]
fn dataframe_to_results(df: &DataFrame) -> Vec<SearchResult> {
    let mut results = Vec::with_capacity(df.height());

    let name_col = df.column("name").ok().and_then(|col| col.str().ok());
    let size_col = df.column("size").ok().and_then(|col| col.u64().ok());
    let frs_col = df.column("frs").ok().and_then(|col| col.u64().ok());
    let flags_col = df.column("flags").ok().and_then(|col| col.u16().ok());

    for idx in 0..df.height() {
        let name = name_col
            .and_then(|col| col.get(idx))
            .unwrap_or("<unknown>")
            .to_owned();
        let size = size_col.and_then(|col| col.get(idx)).unwrap_or(0);
        let frs = frs_col.and_then(|col| col.get(idx)).unwrap_or(0);
        let flags = flags_col.and_then(|col| col.get(idx)).unwrap_or(0);
        let is_directory = (flags & 0x0010) != 0;

        results.push(SearchResult {
            name,
            size,
            frs,
            is_directory,
        });
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigation() {
        let mut app = App::new();
        app.results = vec![
            SearchResult {
                name: "a".to_owned(),
                size: 0,
                frs: 1,
                is_directory: false,
            },
            SearchResult {
                name: "b".to_owned(),
                size: 0,
                frs: 2,
                is_directory: false,
            },
            SearchResult {
                name: "c".to_owned(),
                size: 0,
                frs: 3,
                is_directory: true,
            },
        ];

        app.next();
        assert_eq!(app.list_state.selected(), Some(0));

        app.next();
        assert_eq!(app.list_state.selected(), Some(1));

        app.previous();
        assert_eq!(app.list_state.selected(), Some(0));
    }

    #[test]
    fn test_search_without_index() {
        let mut app = App::new();
        app.input = "test".to_owned();
        app.search();
        // Without an index, search should set an error
        assert!(app.error.is_some());
        assert!(app.results.is_empty());
    }

    #[test]
    fn test_has_index() {
        let app = App::new();
        assert!(!app.has_index());
    }
}
