use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::{COMLibrary, WMIConnection};

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32Volume {
    /// The `Access` property provides information about access rights to the
    /// volume.
    Access: Option<String>,

    /// The `Automount` property indicates whether automount is enabled for the
    /// volume.
    Automount: Option<bool>,

    /// The `Availability` property describes the availability and status of the
    /// volume.
    Availability: Option<String>,

    /// The `BlockSize` property indicates the size of each block, in bytes, on
    /// the volume.
    BlockSize: Option<u64>,

    /// The `BootVolume` property indicates whether the volume is the boot
    /// volume.
    BootVolume: Option<bool>,

    /// The `Capacity` property specifies the total capacity of the volume in
    /// bytes.
    Capacity: Option<u64>,

    /// The `Caption` property contains a short textual description of the
    /// volume.
    Caption: Option<String>,

    /// The `Compressed` property indicates whether the volume is compressed.
    Compressed: Option<bool>,

    /// The `ConfigManagerErrorCode` property contains error codes reported by
    /// the volume.
    ConfigManagerErrorCode: Option<String>,

    /// The `ConfigManagerUserConfig` property contains user configuration
    /// settings for the volume.
    ConfigManagerUserConfig: Option<String>,

    /// The `CreationClassName` property contains the class name used to create
    /// the instance.
    CreationClassName: Option<String>,

    /// The `Description` property provides a textual description of the volume.
    Description: Option<String>,

    /// The `DeviceID` property contains the unique identifier for the volume.
    DeviceID: Option<String>,

    /// The `DirtyBitSet` property indicates whether the dirty bit is set on the
    /// volume.
    DirtyBitSet: Option<bool>,

    /// The `DriveLetter` property contains the letter assigned to the volume
    /// (e.g., C:\).
    DriveLetter: Option<String>,

    /// The `DriveType` property contains an integer that specifies the type of
    /// the volume.
    DriveType: Option<u32>,

    /// The `ErrorCleared` property indicates whether the last error on the
    /// volume has been cleared.
    ErrorCleared: Option<String>,

    /// The `ErrorDescription` property provides a description of the last error
    /// encountered.
    ErrorDescription: Option<String>,

    /// The `ErrorMethodology` property describes the error detection and
    /// correction methodologies.
    ErrorMethodology: Option<String>,

    /// The `FileSystem` property specifies the file system type (e.g., NTFS,
    /// FAT32) of the volume.
    FileSystem: Option<String>,

    /// The `FreeSpace` property indicates the amount of free space, in bytes,
    /// on the volume.
    FreeSpace: Option<u64>,

    /// The `IndexingEnabled` property indicates whether indexing is enabled on
    /// the volume.
    IndexingEnabled: Option<bool>,

    /// The `InstallDate` property contains the date and time when the volume
    /// was installed.
    InstallDate: Option<String>,

    /// The `Label` property contains the volume label.
    Label: Option<String>,

    /// The `LastErrorCode` property provides the last error code encountered by
    /// the volume.
    LastErrorCode: Option<String>,

    /// The `MaximumFileNameLength` property specifies the maximum length of a
    /// file name on the volume.
    MaximumFileNameLength: Option<u32>,

    /// The `Name` property contains the name of the volume.
    Name: Option<String>,

    /// The `NumberOfBlocks` property contains the total number of blocks on the
    /// volume.
    NumberOfBlocks: Option<u64>,

    /// The `PageFilePresent` property indicates whether a page file is present
    /// on the volume.
    PageFilePresent: Option<bool>,

    /// The `PNPDeviceID` property contains the Plug and Play device identifier
    /// for the volume.
    PNPDeviceID: Option<String>,

    /// The `PowerManagementCapabilities` property contains power management
    /// settings.
    PowerManagementCapabilities: Option<Vec<u32>>,

    /// The `PowerManagementSupported` property indicates whether power
    /// management is supported.
    PowerManagementSupported: Option<bool>,

    /// The `Purpose` property specifies the intended purpose of the volume.
    Purpose: Option<String>,

    /// The `QuotasEnabled` property indicates whether disk quotas are enabled
    /// on the volume.
    QuotasEnabled: Option<bool>,

    /// The `QuotasIncomplete` property indicates whether disk quotas are
    /// incomplete on the volume.
    QuotasIncomplete: Option<bool>,

    /// The `QuotasRebuilding` property indicates whether disk quotas are being
    /// rebuilt.
    QuotasRebuilding: Option<bool>,

    /// The `SerialNumber` property contains the serial number of the volume.
    SerialNumber: Option<u32>,

    /// The `Status` property provides the current status of the volume.
    Status: Option<String>,

    /// The `StatusInfo` property provides additional status information.
    StatusInfo: Option<String>,

    /// The `SupportsDiskQuotas` property indicates whether the volume supports
    /// disk quotas.
    SupportsDiskQuotas: Option<bool>,

    /// The `SupportsFileBasedCompression` property indicates whether the volume
    /// supports file-based compression.
    SupportsFileBasedCompression: Option<bool>,

    /// The `SystemCreationClassName` property contains the name of the system
    /// creation class.
    SystemCreationClassName: Option<String>,

    /// The `SystemName` property contains the name of the system the volume
    /// belongs to.
    SystemName: Option<String>,

    /// The `SystemVolume` property indicates whether the volume is the system
    /// volume.
    SystemVolume: Option<bool>,

    /// The `__CLASS` property specifies the WMI class of the object.
    __CLASS: Option<String>,

    /// The `__DERIVATION` property contains the inheritance hierarchy of the
    /// WMI class.
    __DERIVATION: Option<Vec<String>>,

    /// The `__DYNASTY` property specifies the root class in the inheritance
    /// hierarchy.
    __DYNASTY: Option<String>,

    /// The `__GENUS` property is an internal classification value used by WMI.
    __GENUS: Option<i32>,

    /// The `__NAMESPACE` property specifies the WMI namespace where the object
    /// resides.
    __NAMESPACE: Option<String>,

    /// The `__PATH` property contains the full WMI path to the object.
    __PATH: Option<String>,

    /// The `__PROPERTY_COUNT` property indicates the number of properties in
    /// the object.
    __PROPERTY_COUNT: Option<i32>,

    /// The `__RELPATH` property specifies the relative path to the object
    /// within the WMI namespace.
    __RELPATH: Option<String>,

    /// The `__SERVER` property specifies the name of the server where the WMI
    /// object resides.
    __SERVER: Option<String>,

    /// The `__SUPERCLASS` property contains the immediate superclass of the WMI
    /// class.
    __SUPERCLASS: Option<String>,
}

// Implement Display for formatted output
impl fmt::Display for Win32Volume {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(caption) = &self.Caption {
            write!(f, "Caption                     : {}\n", caption)?;
        }
        if let Some(device_id) = &self.DeviceID {
            write!(f, "DeviceID                    : {}\n", device_id)?;
        }
        if let Some(file_system) = &self.FileSystem {
            write!(f, "FileSystem                  : {}\n", file_system)?;
        }
        if let Some(free_space) = &self.FreeSpace {
            write!(f, "FreeSpace                   : {}\n", free_space)?;
        }
        if let Some(capacity) = &self.Capacity {
            write!(f, "Capacity                    : {}\n", capacity)?;
        }
        if let Some(serial_number) = &self.SerialNumber {
            write!(f, "SerialNumber                : {}\n", serial_number)?;
        }
        if let Some(drive_letter) = &self.DriveLetter {
            write!(f, "DriveLetter                 : {}\n", drive_letter)?;
        }
        if let Some(status) = &self.Status {
            write!(f, "Status                      : {}\n", status)?;
        }
        if let Some(indexing_enabled) = &self.IndexingEnabled {
            write!(f, "IndexingEnabled             : {}\n", indexing_enabled)?;
        }
        if let Some(boot_volume) = &self.BootVolume {
            write!(f, "BootVolume                  : {}\n", boot_volume)?;
        }
        if let Some(system_volume) = &self.SystemVolume {
            write!(f, "SystemVolume                : {}\n", system_volume)?;
        }
        if let Some(dirty_bit_set) = &self.DirtyBitSet {
            write!(f, "DirtyBitSet                 : {}\n", dirty_bit_set)?;
        }
        if let Some(label) = &self.Label {
            write!(f, "Label                       : {}\n", label)?;
        }
        if let Some(automount) = &self.Automount {
            write!(f, "Automount                   : {}\n", automount)?;
        }
        if let Some(block_size) = &self.BlockSize {
            write!(f, "BlockSize                   : {}\n", block_size)?;
        }
        if let Some(max_file_name_length) = &self.MaximumFileNameLength {
            write!(
                f,
                "MaximumFileNameLength       : {}\n",
                max_file_name_length
            )?;
        }
        if let Some(pnp_device_id) = &self.PNPDeviceID {
            write!(f, "PNPDeviceID                 : {}\n", pnp_device_id)?;
        }
        if let Some(quotas_enabled) = &self.QuotasEnabled {
            write!(f, "QuotasEnabled               : {}\n", quotas_enabled)?;
        }
        if let Some(quotas_incomplete) = &self.QuotasIncomplete {
            write!(f, "QuotasIncomplete            : {}\n", quotas_incomplete)?;
        }
        if let Some(quotas_rebuilding) = &self.QuotasRebuilding {
            write!(f, "QuotasRebuilding            : {}\n", quotas_rebuilding)?;
        }
        if let Some(supports_disk_quotas) = &self.SupportsDiskQuotas {
            write!(
                f,
                "SupportsDiskQuotas          : {}\n",
                supports_disk_quotas
            )?;
        }
        if let Some(supports_file_based_compression) = &self.SupportsFileBasedCompression {
            write!(
                f,
                "SupportsFileBasedCompression: {}\n",
                supports_file_based_compression
            )?;
        }
        if let Some(__class) = &self.__CLASS {
            write!(f, "__CLASS                     : {}\n", __class)?;
        }
        if let Some(__derivation) = &self.__DERIVATION {
            write!(f, "__DERIVATION                : {:?}\n", __derivation)?;
        }
        if let Some(__dynasty) = &self.__DYNASTY {
            write!(f, "__DYNASTY                   : {}\n", __dynasty)?;
        }
        if let Some(__genus) = &self.__GENUS {
            write!(f, "__GENUS                     : {}\n", __genus)?;
        }
        if let Some(__namespace) = &self.__NAMESPACE {
            write!(f, "__NAMESPACE                 : {}\n", __namespace)?;
        }
        if let Some(__path) = &self.__PATH {
            write!(f, "__PATH                      : {}\n", __path)?;
        }
        if let Some(__property_count) = &self.__PROPERTY_COUNT {
            write!(f, "__PROPERTY_COUNT            : {}\n", __property_count)?;
        }
        if let Some(__relpath) = &self.__RELPATH {
            write!(f, "__RELPATH                   : {}\n", __relpath)?;
        }
        if let Some(__server) = &self.__SERVER {
            write!(f, "__SERVER                    : {}\n", __server)?;
        }
        if let Some(__superclass) = &self.__SUPERCLASS {
            write!(f, "__SUPERCLASS                : {}\n", __superclass)?;
        }
        Ok(())
    }
}

/// Query volumes via WMI (Windows only)
#[cfg(windows)]
pub fn query_volumes() -> Result<Vec<Win32Volume>, UFFSError> {
    // Initialize COM library
    let com_con = COMLibrary::new()
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to initialize COM: {:?}", e)))?;

    // Establish a connection to WMI in the correct namespace
    let wmi_con =
        WMIConnection::with_namespace_path("ROOT\\CIMv2", com_con.into()).map_err(|e| {
            UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
        })?;

    // Define the WMI query
    let query = "SELECT * FROM Win32_Volume";

    // Execute the query and get results
    let results: Vec<Win32Volume> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
