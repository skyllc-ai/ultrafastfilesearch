use std::io;

use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum UFFSError {
    #[error("IO error: {0}")]
    #[diagnostic(
        code(uff::io_error),
        help("Check if the file path is correct and you have the necessary permissions.")
    )]
    Io(#[from] io::Error),

    #[error("Failed to convert VARIANT to u16: {0}")]
    #[diagnostic(code(uff::variant_conversion_error))]
    VariantConversionError(String),

    #[error("Failed to get property: {0}")]
    #[diagnostic(code(uff::get_property_error))]
    GetPropertyError(String),

    #[error("Windows API error: {0}")]
    #[diagnostic(
        code(uff::windows_api_error),
        help("An error occurred while calling a Windows API function.")
    )]
    WindowsApiError(String),

    #[error("WMI query failed: {0}")]
    #[diagnostic(
        code(uff::wmi_query_failed),
        help("Check if the WMI namespace and class are correct.")
    )]
    WMIQueryFailed(String),

    #[error("Drive information is empty.")]
    #[diagnostic(
        code(uff::empty_drive_info),
        help("Ensure that the drive is properly connected and contains data.")
    )]
    EmptyDriveInfo,

    #[error("sysinfo Drive information is empty.")]
    #[diagnostic(
        code(uff::empty_drive_info),
        help("Ensure that the drive(s) are properly connected and contain data.")
    )]
    SysInfoEmptyDriveInfo,

    #[error("Drive letter not found.")]
    #[diagnostic(
        code(uff::drive_letter_not_found),
        help("Verify that the drive letter is correct and accessible.")
    )]
    DriveLetterNotFound,

    #[error("Failed to read directory entries.")]
    #[diagnostic(
        code(uff::directory_read_error),
        help("Check the directory path and your access permissions.")
    )]
    DirectoryReadError,

    #[error("Configuration error: {0}")]
    #[diagnostic(code(uff::config_error))]
    ConfigError(String),

    #[error("Custom error with data: {message}, data: {data}")]
    #[diagnostic(code(uff::custom_error))]
    CustomError { message: String, data: usize },
}
