//! TUI Application state — MftIndex-backed search.

use ratatui::widgets::ListState;

use crate::backend::{DisplayRow, MultiDriveBackend, SortColumn};

/// Application state.
pub struct App {
    /// Current search input.
    pub input: String,
    /// Search results (from last search).
    pub results: Vec<DisplayRow>,
    /// List selection state.
    pub list_state: ListState,
    /// Search backend (multi-drive `MftIndex`).
    pub backend: MultiDriveBackend,
    /// Status message.
    pub status: String,
    /// Error message (if any).
    pub error: Option<String>,
    /// Last search duration in milliseconds.
    pub last_search_ms: u128,
    /// Whether name-only matching is active.
    pub name_only: bool,
    /// Whether a search is currently running in background (Wave 4: spinner).
    #[expect(dead_code, reason = "will be used for UI loading spinner in Wave 4")]
    pub searching: bool,
}

impl App {
    /// Create a new application with a pre-loaded backend.
    #[expect(
        dead_code,
        reason = "public API for synchronous loading; async loading builds incrementally"
    )]
    pub fn with_backend(backend: MultiDriveBackend) -> Self {
        let drive_info = backend
            .drive_summary()
            .iter()
            .map(|(letter, count)| format!("{letter}:{count}"))
            .collect::<Vec<_>>()
            .join(" ");
        let total = backend.total_records();
        let status = format!("Loaded {total} records [{drive_info}]");

        Self {
            input: String::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            backend,
            status,
            error: None,
            last_search_ms: 0,
            name_only: false,
            searching: false,
        }
    }

    /// Create an empty application (no drives loaded).
    pub fn new() -> Self {
        Self {
            input: String::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            backend: MultiDriveBackend::new(),
            status: "No drives loaded. Use --mft-file or --drive to load data.".to_owned(),
            error: None,
            last_search_ms: 0,
            name_only: false,
            searching: false,
        }
    }

    /// Check if any drives are loaded.
    #[must_use]
    pub fn has_data(&self) -> bool {
        !self.backend.drives.is_empty()
    }

    /// Move selection to next item.
    pub fn next(&mut self) {
        let len = self.results.len();
        if len == 0 {
            return;
        }
        let idx = match self.list_state.selected() {
            Some(current) => {
                if current >= len - 1 {
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
        let len = self.results.len();
        if len == 0 {
            return;
        }
        let idx = match self.list_state.selected() {
            Some(current) => {
                if current == 0 {
                    len - 1
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
            self.status = format!(
                "Loaded {} records [{}]",
                self.backend.total_records(),
                self.backend
                    .drive_summary()
                    .iter()
                    .map(|(letter, count)| format!("{letter}:{count}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            return;
        }

        if !self.has_data() {
            self.error = Some("No drives loaded. Use --mft-file or --drive.".to_owned());
            return;
        }

        let result = self.backend.search(&self.input, self.name_only);
        self.last_search_ms = result.duration.as_millis();
        self.results = result.rows;

        // Show trigram debug info
        let tri_info: String = self
            .backend
            .drives
            .iter()
            .map(|dr| format!("{}:{}tri", dr.letter, dr.trigram.posting_count()))
            .collect::<Vec<_>>()
            .join(" ");
        self.status = format!(
            "{} matches in {}ms (scan:{}ms tri_cands:{}) {} records [{}]",
            self.results.len(),
            self.last_search_ms,
            result.scan_ms,
            result.trigram_candidates,
            result.records_scanned,
            tri_info,
        );

        if self.results.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    /// Cycle sort column and re-sort results.
    pub fn cycle_sort(&mut self) {
        self.backend.cycle_sort();
        self.results = self.backend.last_results.clone();
    }

    /// Toggle sort direction and re-sort results.
    pub fn toggle_sort_direction(&mut self) {
        self.backend.toggle_sort_direction();
        self.results = self.backend.last_results.clone();
    }

    /// Get the current sort column.
    #[must_use]
    pub const fn sort_column(&self) -> SortColumn {
        self.backend.sort_column
    }

    /// Get whether sort is descending.
    #[must_use]
    pub const fn sort_desc(&self) -> bool {
        self.backend.sort_desc
    }

    /// Toggle name-only matching mode.
    pub const fn toggle_name_only(&mut self) {
        self.name_only = !self.name_only;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigation() {
        let mut app = App::new();
        app.results = vec![
            DisplayRow {
                drive: 'C',
                path: "C:\\a".to_owned(),
                name: "a".to_owned(),
                size: 0,
                is_directory: false,
                modified: 0,
            },
            DisplayRow {
                drive: 'C',
                path: "C:\\b".to_owned(),
                name: "b".to_owned(),
                size: 0,
                is_directory: false,
                modified: 0,
            },
            DisplayRow {
                drive: 'C',
                path: "C:\\c".to_owned(),
                name: "c".to_owned(),
                size: 0,
                is_directory: true,
                modified: 0,
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
    fn test_search_without_data() {
        let mut app = App::new();
        app.input = "test".to_owned();
        app.search();
        assert!(app.error.is_some());
        assert!(app.results.is_empty());
    }

    #[test]
    fn test_has_data() {
        let app = App::new();
        assert!(!app.has_data());
    }

    #[test]
    fn test_empty_search_clears_results() {
        let mut app = App::new();
        app.results = vec![DisplayRow {
            drive: 'C',
            path: "C:\\x".to_owned(),
            name: "x".to_owned(),
            size: 0,
            is_directory: false,
            modified: 0,
        }];
        app.input.clear();
        app.search();
        assert!(app.results.is_empty());
    }
}
