use core::time::Duration;
use std::ffi::{OsStr, OsString};
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
#[cfg(windows)]
use std::os::windows::prelude::OsStrExt;
use std::path::{Component, Path};

use colored::Colorize;
use sysinfo::Disks;
#[cfg(windows)]
use windows::Win32::Foundation::GetLastError;
#[cfg(windows)]
use windows::core::PCWSTR;

use crate::modules::entities::disk::{ColumnLengths, DriveInfo, DriveType};
use crate::modules::errors::UFFSError;
use crate::modules::utils::format_utils::{format_number, format_size};
use crate::modules::utils::string_utils::to_wide_string_with_null;
use crate::modules::utils::time_utils::format_duration;

fn get_drive_letter(path: &Path) -> Option<OsString> {
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        if let std::path::Prefix::Disk(drive) = prefix.kind() {
            // Convert drive letter (u8) to OsString and append `:`
            let mut drive_letter = OsString::new();
            drive_letter.push(format!("{}:", drive as char));
            return Some(drive_letter);
        }
    }
    None
}

pub fn get_drive_info() -> anyhow::Result<Vec<DriveInfo>, UFFSError> {
    // Attempt to create a new Disks object
    let disks = Disks::new_with_refreshed_list();

    // Check if the list of disks is empty and return an appropriate error
    if disks.is_empty() {
        return Err(UFFSError::SysInfoEmptyDriveInfo);
    }

    let mut all_disks = Vec::new();

    for disk in disks.iter() {
        let mut drive_info = DriveInfo {
            root_path: OsString::from(disk.mount_point()),
            total_space: disk.total_space() as usize,
            available_space: disk.available_space() as usize,
            drive_type: DriveType::from(disk.kind()),
            drive_name: disk.name().to_os_string(),
            file_system_type: None,
            num_files: 0,
            num_dirs: 0,
            time_nanoseconds: 0,
            uuid: None,
            serial_number: None,
            mount_point: None,
            sector_size: None,
            block_size: None,
            cylinders: None,
            tracks_per_cylinder: None,
            sectors_per_track: None,
            bytes_per_sector: None,
            #[cfg(windows)]
            media_type: None,
            volume_serial_number: None,
            volume_name: None,
            ntfs_version: None,
            physical_device_path: None,
            is_bitlocker_encrypted: None,
            max_component_length: None,
            drive_letter: get_drive_letter(disk.mount_point()),
            drive_root: None,
            dos_device_name: None,
            device_path: None,
            volume_guid_path: None,
            mounted_folder_path: None,
            unc_path: None,
            device_identifier: None,
            container_size: None,
            volume_role: None,
            apfs_version: None,
            device_node: None,
            raid_info: None,
            inode_count: None,
            is_removable: false,
            mount_options: None,
        };

        collect_drive_info(&mut drive_info);

        drive_info.print_as_json();

        all_disks.push(drive_info);

        if true {
            break;
        }
    }

    // Return the vector of disks if successfully collected
    Ok(all_disks)
}

pub fn print_drive_info_table(drives: &mut [DriveInfo], total_duration: Duration) {
    // Define column lengths (or use defaults)
    let mut lengths = ColumnLengths::default();

    let total_formatted_duration = format_duration(total_duration);

    // Accumulate totals and find the longest path length in one pass using fold
    let (longest_path_length, total_files, total_dirs, total_space, available_space) =
        drives.iter().fold(
            (0, 0, 0, 0, 0),
            |(longest, files, dirs, space, available), drive| {
                let path_len = drive.root_path.to_string_lossy().len();
                (
                    longest.max(path_len),             // Find the maximum path length
                    files + drive.num_files,           // Sum the number of files
                    dirs + drive.num_dirs,             // Sum the number of directories
                    space + drive.total_space,         // Sum the total space
                    available + drive.available_space, // Sum the available space
                )
            },
        );

    let type_length = lengths.type_length;
    let total_space_length = lengths.total_space_length;
    let available_space_length = lengths.available_space_length;
    let files_length = lengths.files_length;
    let dirs_length = lengths.dirs_length;
    let time_length = lengths.time_length;
    let time_seconds_length = lengths.time_seconds_length;

    // Header
    println!(
        "{:<longest_path_length$}   {:<type_length$}     {:>total_space_length$}   {:>available_space_length$}   {:>files_length$}   {:>dirs_length$}   {:>time_seconds_length$}  {:>time_length$}",
        "Path".bold().underline().blue(),
        "Type".bold().underline().blue(),
        "Total Size".bold().underline().blue(),
        "Available Space".bold().underline().blue(),
        "Files".bold().underline().blue(),
        "Dirs".bold().underline().blue(),
        "Time (s)".bold().underline().blue(),
        "Time".bold().underline().blue(),
    );

    // Separator
    let separator = format!(
        "{}",
        "-".repeat(
            longest_path_length
                + type_length
                + total_space_length
                + available_space_length
                + files_length
                + dirs_length
                + time_seconds_length
                + time_length
                + 7 * 4
        )
        .green()
    );
    println!("{}", separator);

    lengths.path_length = longest_path_length;

    // Sort drives by root_path (converted to a string for sorting)
    drives.sort_by_key(|drive| drive.root_path.to_string_lossy().to_string());

    // Print each drive using the Display implementation
    for drive in drives {
        println!("{}", drive.format_with_lengths(&lengths));
    }

    // Separator
    println!("{}", separator);

    // Total row
    println!(
        "{:<longest_path_length$}   {:<type_length$}   {:>total_space_length$}   {:>available_space_length$}   {:>files_length$}   {:>dirs_length$}   {:>time_seconds_length$.3}  {:>time_length$}",
        "Total".bold().yellow(),
        "",
        format_size(total_space),
        format_size(available_space),
        format_number(total_files, files_length),
        format_number(total_dirs, dirs_length),
        total_duration.as_secs_f64(),
        total_formatted_duration,
    );
    println!("\n");
}

// Function that collects disk information based on the platform
pub fn collect_drive_info(mut drive: &mut DriveInfo) {
    // Platform-specific data collection
    #[cfg(target_os = "linux")]
    collect_linux_specific_info(&mut drive);

    #[cfg(target_os = "windows")]
    collect_windows_specific_info(&mut drive);

    #[cfg(target_os = "macos")]
    collect_macos_specific_info(&mut drive);

    drive.print_as_json();
}

#[cfg(target_os = "linux")]
fn collect_linux_specific_info(drive: &mut DriveInfo) {
    use std::process::Command;

    // Collect UUID using blkid
    if let Ok(output) = Command::new("blkid").arg(&drive.root_path).output() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        if let Some(uuid) = output_str.split("UUID=\"").nth(1) {
            drive.uuid = Some(uuid.split('"').next().unwrap_or("").to_string());
        }
    }

    // Collect sector size and block size from /sys
    let device = drive.drive_name.to_str().unwrap_or("");
    let sector_size_path = format!("/sys/block/{}/queue/hw_sector_size", device);
    if let Ok(sector_size) = std::fs::read_to_string(sector_size_path) {
        drive.sector_size = Some(sector_size.trim().parse::<u64>().unwrap_or(0));
    }

    let block_size_path = format!("/sys/block/{}/queue/logical_block_size", device);
    if let Ok(block_size) = std::fs::read_to_string(block_size_path) {
        drive.block_size = Some(block_size.trim().parse::<u64>().unwrap_or(0));
    }

    // Collect serial number from /sys
    let serial_path = format!("/sys/block/{}/device/serial", device);
    if let Ok(serial_number) = std::fs::read_to_string(serial_path) {
        drive.serial_number = Some(serial_number.trim().to_string());
    }
}

/// Use `QueryDosDeviceW` to get the device name from a mount point (Windows
/// only)
#[cfg(windows)]
fn query_dos_device(mount_point: &OsStr) -> Option<Vec<u16>> {
    use windows::Win32::Storage::FileSystem::QueryDosDeviceW;

    let mut buffer: Vec<u16> = vec![0; 1024]; // Buffer to hold the device name

    // Convert mount point (e.g., "C:") to a wide string
    let mount_point_wide = to_wide_string_with_null(mount_point);

    // Call QueryDosDeviceW
    let result = unsafe {
        QueryDosDeviceW(
            PCWSTR(mount_point_wide.as_ptr()), // Pass the wide string pointer
            Some(&mut buffer),                 // Output buffer
        )
    };

    if result != 0 {
        // Find where the null terminator is in the buffer
        let end = buffer.iter().position(|&x| x == 0).unwrap_or(buffer.len());
        Some(buffer[..end].to_vec()) // Return the part of the buffer that contains the device name
    } else {
        // Get the last error using GetLastError
        let error = unsafe { GetLastError() };
        eprintln!("QueryDosDeviceW failed with error code: {}", error.0);
        None
    }
}

// fn print_file_system_flags(flags: u32) {
//     println!("Decoded File System Flags:");
//
//     if flags & FILE_CASE_SENSITIVE_SEARCH != 0 {
//         println!("- Case Sensitive Search");
//     }
//     if flags & FILE_CASE_PRESERVED_NAMES != 0 {
//         println!("- Case Preserved Names");
//     }
//     if flags & FILE_UNICODE_ON_DISK != 0 {
//         println!("- Supports Unicode on Disk");
//     }
//     if flags & FILE_PERSISTENT_ACLS != 0 {
//         println!("- Persistent ACLs");
//     }
//     if flags & FILE_FILE_COMPRESSION != 0 {
//         println!("- Supports File Compression");
//     }
//     if flags & FILE_VOLUME_QUOTAS != 0 {
//         println!("- Supports Volume Quotas");
//     }
//     if flags & FILE_SUPPORTS_SPARSE_FILES != 0 {
//         println!("- Supports Sparse Files");
//     }
//     if flags & FILE_SUPPORTS_REPARSE_POINTS != 0 {
//         println!("- Supports Reparse Points");
//     }
//     if flags & FILE_SUPPORTS_OBJECT_IDS != 0 {
//         println!("- Supports Object IDs");
//     }
//     if flags & FILE_SUPPORTS_ENCRYPTION != 0 {
//         println!("- Supports Encryption");
//     }
//     if flags & FILE_NAMED_STREAMS != 0 {
//         println!("- Supports Named Streams");
//     }
//     if flags & FILE_READ_ONLY_VOLUME != 0 {
//         println!("- Read-Only Volume");
//     }
//     if flags & FILE_SEQUENTIAL_WRITE_ONCE != 0 {
//         println!("- Sequential Write Once");
//     }
//     if flags & FILE_SUPPORTS_TRANSACTIONS != 0 {
//         println!("- Supports Transactions");
//     }
//     if flags & FILE_SUPPORTS_HARD_LINKS != 0 {
//         println!("- Supports Hard Links");
//     }
//     if flags & FILE_SUPPORTS_EXTENDED_ATTRIBUTES != 0 {
//         println!("- Supports Extended Attributes");
//     }
//     if flags & FILE_SUPPORTS_OPEN_BY_FILE_ID != 0 {
//         println!("- Supports Open by File ID");
//     }
//     if flags & FILE_SUPPORTS_USN_JOURNAL != 0 {
//         println!("- Supports USN Journal");
//     }
//     if flags & FILE_VOLUME_IS_COMPRESSED != 0 {
//         println!("- Volume Is Compressed");
//     }
//
//     if flags == 0 {
//         println!("- No special features.");
//     }
// }

/// Converts a mount point (e.g., "C:\\" or "C:") into the proper `\\.\C:`
/// format and returns a null-terminated wide string (`Vec<u16>`) for use with
/// Windows APIs.
#[cfg(windows)]
fn convert_to_device_path(mount_point: &OsStr) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::prelude::OsStrExt;

    // Convert OsStr to a wide string (UTF-16)
    let mut cleaned_mount_point: Vec<u16> = mount_point.encode_wide().collect();

    // Remove trailing backslashes
    while let Some(&last_char) = cleaned_mount_point.last() {
        if last_char == b'\\' as u16 {
            cleaned_mount_point.pop();
        } else {
            break;
        }
    }

    // Format the path as \\.\C: or similar
    let mut formatted_path = vec![b'\\' as u16, b'\\' as u16, b'.' as u16, b'\\' as u16]; // Starts with \\.\

    // Append the cleaned mount point (already in wide string format)
    formatted_path.extend_from_slice(&cleaned_mount_point);

    // Null terminate the wide string for Windows API compatibility
    formatted_path.push(0);

    OsString::from_wide(&formatted_path)
}

use serde::Deserialize;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
#[cfg(windows)]
use wmi::WMIConnection;

#[cfg(windows)]
#[derive(Deserialize, Debug)]
struct Win32_EncryptableVolume {
    DeviceID: String,
    ProtectionStatus: u32, // 1 means encrypted
}

#[cfg(windows)]
fn is_bitlocker_encrypted(device_id: &OsString) -> Option<bool> {
    // Establish a connection to WMI and specify the correct namespace
    let wmi_con =
        WMIConnection::with_namespace_path("ROOT\\CIMv2\\Security\\MicrosoftVolumeEncryption")
            .unwrap();

    let query = "SELECT * FROM Win32_EncryptableVolume";

    let results: Vec<Win32_EncryptableVolume> = wmi_con.raw_query(query).unwrap();

    // Iterate through the results and print properties
    for volume in results {
        println!("DeviceID: {}", volume.DeviceID);
        println!("ProtectionStatus: {}", volume.ProtectionStatus);
        println!("volume: {:?}", volume);
    }

    Some(true)
}

#[cfg(target_os = "windows")]
fn collect_windows_specific_info(drive: &mut DriveInfo) {
    drive.mount_point = Some(drive.root_path.to_os_string());

    let device_name = query_dos_device(drive.drive_letter.clone().unwrap().as_os_str()).unwrap();
    println!(
        "Device Name (resolved by QueryDosDeviceW): {:?}",
        String::from_utf16_lossy(&device_name).trim_end_matches('\0')
    );

    drive.dos_device_name = Some(OsString::from_wide(&device_name));

    drive.physical_device_path = Some(OsString::from_wide(&device_name));

    drive.device_path = Some(convert_to_device_path(drive.root_path.as_os_str()));

    // drive.volume_guid_path =
    // Some(get_volume_guid_path(&drive.drive_letter.clone().unwrap()).unwrap());
    drive.is_bitlocker_encrypted = is_bitlocker_encrypted(&drive.volume_guid_path.clone().unwrap());
}

#[cfg(target_os = "macos")]
fn collect_macos_specific_info(drive: &mut DriveInfo) {
    use std::process::Command;

    // Use diskutil to get UUID and Serial number
    if let Ok(output) = Command::new("diskutil")
        .arg("info")
        .arg(&drive.root_path)
        .output()
    {
        let output_str = String::from_utf8_lossy(&output.stdout);
        if let Some(uuid) = output_str.split("Volume UUID: ").nth(1) {
            drive.uuid = Some(uuid.split_whitespace().next().unwrap_or("").to_string());
        }

        if let Some(serial) = output_str.split("Disk Serial Number: ").nth(1) {
            drive.serial_number = Some(serial.split_whitespace().next().unwrap_or("").to_string());
        }
    }

    // Use `diskutil` or other system commands to get sector size and block size
    // if needed.
}
