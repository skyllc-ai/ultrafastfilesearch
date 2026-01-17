use num_format::{Locale, ToFormattedString};

pub(crate) fn format_size(size: usize) -> String {
    let size_gb = size as f64 / (1024.0 * 1024.0 * 1024.0);
    if size_gb >= 1024.0 {
        format!("{:>9.2} TB", size_gb / 1024.0)
    } else {
        format!("{:>9.2} GB", size_gb)
    }
}

pub(crate) fn format_number(number: usize, width: usize) -> String {
    let formatted_number = number.to_formatted_string(&Locale::en);
    format!("{:>width$}", formatted_number)
}
