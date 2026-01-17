use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

pub fn print_directory_tree<P: AsRef<Path>>(path: P) -> io::Result<()> {
    println!("\n");

    let path = path.as_ref();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    writeln!(handle, "{}", path.display())?;

    // Collect all entries (path and filename) into a vector
    let ignore_walker = WalkBuilder::new(path)
        .git_ignore(true) // Respect .gitignore
        .hidden(false) // Skip hidden files
        .build();

    let mut filtered_entries: HashSet<PathBuf> = ignore_walker
        .filter_map(|entry| entry.ok()) // Collect valid entries
        .map(|entry| {
            let path = entry.path().to_path_buf();
            path
        })
        .collect(); // Collect into a HashSet

    // Collect all entries (path and filename) into a vector
    let all_walker = WalkBuilder::new(path)
        .git_ignore(false) // Respect .gitignore
        .hidden(false) // Skip hidden files
        .build();

    let mut all_entries: HashSet<PathBuf> = all_walker
        .filter_map(|entry| entry.ok()) // Collect valid entries
        .map(|entry| {
            let path = entry.path().to_path_buf();
            let _filename = entry.file_name().to_string_lossy().to_string();
            path
        })
        .collect();

    // Find the symmetric difference between the two sets
    let mut diff: Vec<_> = all_entries
        .symmetric_difference(&filtered_entries)
        .cloned() // Clone the values from the set (required by Rust since symmetric_difference() returns
        // references)
        .collect();

    let mut ignore_path = Path::new("C:\\Users\\rnio\\GitHub\\UltraFastFileSearch\\.git");
    diff.push(PathBuf::from(ignore_path));
    ignore_path = Path::new("C:\\Users\\rnio\\GitHub\\UltraFastFileSearch\\.gitignore");
    diff.push(PathBuf::from(ignore_path));

    // println!("\nDIFF:\t{:?}\n\n", diff);

    print_directory_tree_recursive(path, "", &mut handle, &diff)?;

    println!("\n");

    Ok(())
}

pub(crate) fn print_directory_tree_recursive<P: AsRef<Path>>(
    path: P,
    prefix: &str,
    handle: &mut impl Write,
    diff: &Vec<PathBuf>,
) -> io::Result<()> {
    let path = path.as_ref();
    let entries = fs::read_dir(path)?;

    let mut entries: Vec<_> = entries.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for (i, entry) in entries.iter().enumerate() {
        let entry_path = entry.path().to_path_buf();

        // Check if the entry path is not in the diff set
        if !diff.contains(&entry_path) {
            let path = entry.path();
            let metadata = fs::metadata(&path)?;

            let is_last = i == entries.len() - 1;
            let new_prefix = if is_last { "└── " } else { "├── " };
            let continuation_prefix = if is_last { "    " } else { "│   " };

            writeln!(
                handle,
                "{}{}{}",
                prefix,
                new_prefix,
                entry.file_name().to_string_lossy()
            )?;

            if metadata.is_dir() {
                print_directory_tree_recursive(
                    &path,
                    &format!("{}{}", prefix, continuation_prefix),
                    handle,
                    &diff,
                )?;
            }
        }
    }

    Ok(())
}
