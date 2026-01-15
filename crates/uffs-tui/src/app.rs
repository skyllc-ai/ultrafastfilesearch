//! TUI Application state.

use ratatui::widgets::ListState;

/// A search result item.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// File or directory name.
    pub name: String,
    /// File size in bytes.
    pub size: u64,
    /// Full path to the file.
    #[allow(dead_code)] // Used in TUI rendering (TODO)
    pub path: String,
}

/// Application state.
pub struct App {
    /// Current search input.
    pub input: String,
    /// Search results.
    pub results: Vec<SearchResult>,
    /// List selection state.
    pub list_state: ListState,
}

impl App {
    /// Create a new application.
    pub fn new() -> Self {
        Self {
            input: String::new(),
            results: Vec::new(),
            list_state: ListState::default(),
        }
    }

    /// Move selection to next item.
    pub fn next(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.results.len().saturating_sub(1) {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    /// Move selection to previous item.
    pub fn previous(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.results.len().saturating_sub(1)
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    /// Execute search with current input.
    pub fn search(&mut self) {
        // TODO: Implement actual search using uffs-core
        // For now, show placeholder results
        if self.input.is_empty() {
            self.results.clear();
            return;
        }

        // Placeholder results for demonstration
        self.results = vec![
            SearchResult {
                name: format!("example_{}.txt", self.input),
                size: 1024,
                path: format!("C:\\Users\\example_{}.txt", self.input),
            },
            SearchResult {
                name: format!("test_{}.rs", self.input),
                size: 2048,
                path: format!("C:\\Projects\\test_{}.rs", self.input),
            },
        ];

        self.list_state.select(Some(0));
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
            SearchResult {
                name: "a".to_string(),
                size: 0,
                path: String::new(),
            },
            SearchResult {
                name: "b".to_string(),
                size: 0,
                path: String::new(),
            },
            SearchResult {
                name: "c".to_string(),
                size: 0,
                path: String::new(),
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
    fn test_search() {
        let mut app = App::new();
        app.input = "test".to_string();
        app.search();
        assert!(!app.results.is_empty());
    }
}

