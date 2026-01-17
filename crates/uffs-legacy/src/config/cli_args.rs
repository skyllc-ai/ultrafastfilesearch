use clap::{Parser, ValueEnum};

#[derive(Parser)]
#[command(
    name = "uffs",
    version = "1.0",
    about = "UFFS - Ultra Fast File Search. Locate files and folders by name instantly."
)]
pub struct Cli {
    /// Search path. E.g. 'C:/' or 'C:/Prog*'
    #[arg(long, short, default_value = "*")]
    pub search_path: String,

    // Add other CLI options here
    #[arg(long, value_enum, default_value = "path,name")]
    pub columns: Vec<Columns>,
}

#[derive(ValueEnum, Clone)]
pub enum Columns {
    Path,
    Name,
    // Other columns...
}

pub fn parse_cli() -> Cli {
    Cli::parse()
}
