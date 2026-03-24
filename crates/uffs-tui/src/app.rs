//! TUI Application state — MftIndex-backed search.

use ratatui::widgets::TableState;
use ratatui_textarea::TextArea;

use crate::backend::{DisplayRow, FilterMode, MultiDriveBackend, SortColumn};

/// Application state.
pub struct App {
    /// Search input text area (full editing: cursor, selection, clipboard).
    pub textarea: TextArea<'static>,
    /// Search results (from last search).
    pub results: Vec<DisplayRow>,
    /// Table selection state.
    pub table_state: TableState,
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
    /// Filter mode: `All`, `FilesOnly`, or `DirsOnly`.
    pub filter_mode: FilterMode,
    /// Whether a search is currently running in background (Wave 4: spinner).
    #[expect(dead_code, reason = "will be used for UI loading spinner in Wave 4")]
    pub searching: bool,
    /// Visible page size for PageUp/Down (set by `ui()` on each render).
    pub page_size: usize,
}

impl App {
    /// Get the current search text from the textarea.
    pub fn input_text(&self) -> String {
        self.textarea
            .lines()
            .first()
            .map_or(String::new(), ToOwned::to_owned)
    }

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
            textarea: make_search_textarea(),
            results: Vec::new(),
            table_state: TableState::default(),
            backend,
            status,
            error: None,
            last_search_ms: 0,
            name_only: false,
            filter_mode: FilterMode::All,
            searching: false,
            page_size: 20,
        }
    }

    /// Create an empty application (no drives loaded).
    pub fn new() -> Self {
        Self {
            textarea: make_search_textarea(),
            results: Vec::new(),
            table_state: TableState::default(),
            backend: MultiDriveBackend::new(),
            status: "No drives loaded. Use --mft-file or --drive to load data.".to_owned(),
            error: None,
            last_search_ms: 0,
            name_only: false,
            filter_mode: FilterMode::All,
            searching: false,
            page_size: 20,
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
        let idx = match self.table_state.selected() {
            Some(current) => {
                if current >= len - 1 {
                    0
                } else {
                    current + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(idx));
    }

    /// Move selection to previous item.
    pub fn previous(&mut self) {
        let len = self.results.len();
        if len == 0 {
            return;
        }
        let idx = match self.table_state.selected() {
            Some(current) => {
                if current == 0 {
                    len - 1
                } else {
                    current - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(idx));
    }

    /// Move selection down by one visible page.
    pub fn page_down(&mut self) {
        let len = self.results.len();
        if len == 0 {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0);
        let new_idx = (current + self.page_size).min(len - 1);
        self.table_state.select(Some(new_idx));
    }

    /// Move selection up by one visible page.
    pub fn page_up(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0);
        let new_idx = current.saturating_sub(self.page_size);
        self.table_state.select(Some(new_idx));
    }

    /// Get the full path of the currently selected result.
    #[must_use]
    pub fn selected_path(&self) -> Option<&str> {
        let idx = self.table_state.selected()?;
        self.results.get(idx).map(|row| row.path.as_str())
    }

    /// Execute search with current input.
    pub fn search(&mut self) {
        self.error = None;
        let input = self.input_text();

        if input.is_empty() {
            self.results.clear();
            let fc = |n: usize| uffs_mft::format_number_commas(n as u64);
            let drive_info: String = self
                .backend
                .drive_summary()
                .iter()
                .map(|(letter, count)| format!("{letter}:{}", fc(*count)))
                .collect::<Vec<_>>()
                .join("  ");
            self.status = format!(
                "Loaded {} records  │  {} drives  [{}]",
                fc(self.backend.total_records()),
                self.backend.drives.len(),
                drive_info,
            );
            return;
        }

        if !self.has_data() {
            self.error = Some("No drives loaded. Use --mft-file or --drive.".to_owned());
            return;
        }

        let result = self.backend.search(&input, self.name_only);
        self.last_search_ms = result.duration.as_millis();
        self.results = result.rows;
        crate::backend::apply_filter(&mut self.results, self.filter_mode);

        let fc = |n: usize| uffs_mft::format_number_commas(n as u64);
        let total_trigrams: usize = self
            .backend
            .drives
            .iter()
            .map(|dr| dr.trigram.posting_count())
            .sum();
        self.status = format!(
            "{} matches  │  {}  │  {} records across {} drives  │  {} trigrams",
            fc(self.results.len()),
            {
                let ms = result.duration.as_millis();
                if ms < 1000 {
                    format!("{ms}ms")
                } else {
                    let tenths = (ms + 50) / 100;
                    let whole = tenths / 10;
                    let frac = tenths % 10;
                    format!("{whole}.{frac}s")
                }
            },
            fc(result.records_scanned),
            self.backend.drives.len(),
            fc(total_trigrams),
        );

        if self.results.is_empty() {
            self.table_state.select(None);
        } else {
            self.table_state.select(Some(0));
        }
    }

    /// Cycle sort column and re-sort results.
    pub fn cycle_sort(&mut self) {
        self.backend.cycle_sort();
        self.results = self.backend.last_results.clone();
        crate::backend::apply_filter(&mut self.results, self.filter_mode);
    }

    /// Toggle sort direction and re-sort results.
    pub fn toggle_sort_direction(&mut self) {
        self.backend.toggle_sort_direction();
        self.results = self.backend.last_results.clone();
        crate::backend::apply_filter(&mut self.results, self.filter_mode);
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

    /// Cycle filter mode: `All` → `FilesOnly` → `DirsOnly` → `All`.
    pub const fn cycle_filter(&mut self) {
        self.filter_mode = match self.filter_mode {
            FilterMode::All => FilterMode::FilesOnly,
            FilterMode::FilesOnly => FilterMode::DirsOnly,
            FilterMode::DirsOnly => FilterMode::All,
        };
    }

    /// Get a display label for the current filter mode.
    #[must_use]
    pub const fn filter_label(&self) -> &str {
        match self.filter_mode {
            FilterMode::All => "",
            FilterMode::FilesOnly => " [FILES]",
            FilterMode::DirsOnly => " [DIRS]",
        }
    }
}

/// Create a configured single-line `TextArea` for the search box.
fn make_search_textarea<'a>() -> TextArea<'a> {
    use ratatui::style::{Color, Style};

    let mut textarea = TextArea::default();
    textarea.set_cursor_line_style(Style::default());
    textarea.set_style(Style::default().fg(Color::Yellow));
    textarea.set_placeholder_text("Type to search...");
    textarea.set_block(ratatui::widgets::Block::default());
    textarea
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
        assert_eq!(app.table_state.selected(), Some(0));

        app.next();
        assert_eq!(app.table_state.selected(), Some(1));

        app.previous();
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_search_without_data() {
        let mut app = App::new();
        app.textarea.insert_str("test");
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
        // textarea starts empty by default, search should clear results
        app.search();
        assert!(app.results.is_empty());
    }
}
