use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::WMIConnection;

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32DiskPartition {
    /// The `Access` property indicates the access rights to the partition.
    Access: Option<u16>,

    /// The `Availability` property indicates the availability and status of the
    /// device.
    Availability: Option<u16>,

    /// The `BlockSize` property specifies the block size in bytes.
    BlockSize: Option<u64>,

    /// The `Bootable` property indicates whether the partition is bootable.
    Bootable: Option<bool>,

    /// The `BootPartition` property indicates whether the partition is the
    /// active (boot) partition.
    BootPartition: Option<bool>,

    /// The `Caption` property provides a short textual description of the
    /// partition.
    Caption: Option<String>,

    /// The `ConfigManagerErrorCode` property contains an error code, if
    /// applicable.
    ConfigManagerErrorCode: Option<u32>,

    /// The `ConfigManagerUserConfig` property indicates whether the partition
    /// is user-configurable.
    ConfigManagerUserConfig: Option<bool>,

    /// The `CreationClassName` property contains the name of the class or
    /// subclass used to create the partition.
    CreationClassName: Option<String>,

    /// The `Description` property contains a description of the partition.
    Description: Option<String>,

    /// The `DeviceID` property contains the unique identifier for the
    /// partition.
    DeviceID: String,

    /// The `DiskIndex` property specifies the index of the disk to which this
    /// partition belongs.
    DiskIndex: Option<u32>,

    /// The `ErrorCleared` property indicates whether the last error was
    /// cleared.
    ErrorCleared: Option<bool>,

    /// The `ErrorDescription` property contains a description of the last
    /// error.
    ErrorDescription: Option<String>,

    /// The `ErrorMethodology` property describes the error detection and
    /// correction methods.
    ErrorMethodology: Option<String>,

    /// The `HiddenSectors` property indicates the number of hidden sectors in
    /// the partition.
    HiddenSectors: Option<u32>,

    /// The `Index` property specifies the index of the partition.
    Index: Option<u32>,

    /// The `InstallDate` property contains the installation date of the
    /// partition.
    InstallDate: Option<String>,

    /// The `LastErrorCode` property contains the last error code.
    LastErrorCode: Option<u32>,

    /// The `Name` property contains the name of the partition.
    Name: Option<String>,

    /// The `NumberOfBlocks` property indicates the total number of blocks in
    /// the partition.
    NumberOfBlocks: Option<u64>,

    /// The `PNPDeviceID` property contains the Plug and Play device identifier.
    PNPDeviceID: Option<String>,

    /// The `PowerManagementCapabilities` property contains power management
    /// features of the partition.
    PowerManagementCapabilities: Option<Vec<u16>>,

    /// The `PowerManagementSupported` property indicates whether power
    /// management is supported.
    PowerManagementSupported: Option<bool>,

    /// The `PrimaryPartition` property indicates whether the partition is a
    /// primary partition.
    PrimaryPartition: Option<bool>,

    /// The `Purpose` property describes the purpose of the partition.
    Purpose: Option<String>,

    /// The `RewritePartition` property indicates whether the partition can be
    /// rewritten.
    RewritePartition: Option<bool>,

    /// The `Size` property specifies the size of the partition in bytes.
    Size: Option<u64>,

    /// The `StartingOffset` property specifies the starting offset of the
    /// partition.
    StartingOffset: Option<u64>,

    /// The `Status` property contains the current status of the partition.
    Status: Option<String>,

    /// The `StatusInfo` property provides additional status information.
    StatusInfo: Option<u16>,

    /// The `SystemCreationClassName` property contains the name of the class
    /// used to create the partition's system.
    SystemCreationClassName: Option<String>,

    /// The `SystemName` property contains the name of the system to which this
    /// partition belongs.
    SystemName: Option<String>,

    /// The `Type` property contains the type of partition (e.g., "GPT: Basic
    /// Data").
    Type: Option<String>,

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

// Implement Display for a nicer output format
impl fmt::Display for Win32DiskPartition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(caption) = &self.Caption {
            write!(f, "Caption                      : {}\n", caption)?;
        }
        if let Some(description) = &self.Description {
            write!(f, "Description                  : {}\n", description)?;
        }
        write!(f, "DeviceID                     : {}\n", self.DeviceID)?;
        if let Some(disk_index) = &self.DiskIndex {
            write!(f, "DiskIndex                    : {}\n", disk_index)?;
        }
        if let Some(block_size) = &self.BlockSize {
            write!(f, "BlockSize                    : {}\n", block_size)?;
        }
        if let Some(bootable) = &self.Bootable {
            write!(f, "Bootable                     : {}\n", bootable)?;
        }
        if let Some(boot_partition) = &self.BootPartition {
            write!(f, "BootPartition                : {}\n", boot_partition)?;
        }
        if let Some(size) = &self.Size {
            write!(f, "Size                         : {}\n", size)?;
        }
        if let Some(primary_partition) = &self.PrimaryPartition {
            write!(f, "PrimaryPartition             : {}\n", primary_partition)?;
        }
        if let Some(starting_offset) = &self.StartingOffset {
            write!(f, "StartingOffset               : {}\n", starting_offset)?;
        }
        if let Some(status) = &self.Status {
            write!(f, "Status                       : {}\n", status)?;
        }
        if let Some(system_creation_class_name) = &self.SystemCreationClassName {
            write!(
                f,
                "SystemCreationClassName      : {}\n",
                system_creation_class_name
            )?;
        }
        if let Some(system_name) = &self.SystemName {
            write!(f, "SystemName                   : {}\n", system_name)?;
        }
        if let Some(partition_type) = &self.Type {
            write!(f, "Type                         : {}\n", partition_type)?;
        }
        if let Some(class) = &self.__CLASS {
            write!(f, "__CLASS                      : {}\n", class)?;
        }
        if let Some(derivation) = &self.__DERIVATION {
            write!(f, "__DERIVATION                 : {:?}\n", derivation)?;
        }
        if let Some(dynasty) = &self.__DYNASTY {
            write!(f, "__DYNASTY                    : {}\n", dynasty)?;
        }
        if let Some(genus) = &self.__GENUS {
            write!(f, "__GENUS                      : {}\n", genus)?;
        }
        if let Some(namespace) = &self.__NAMESPACE {
            write!(f, "__NAMESPACE                  : {}\n", namespace)?;
        }
        if let Some(path) = &self.__PATH {
            write!(f, "__PATH                       : {}\n", path)?;
        }
        if let Some(property_count) = &self.__PROPERTY_COUNT {
            write!(f, "__PROPERTY_COUNT             : {}\n", property_count)?;
        }
        if let Some(relpath) = &self.__RELPATH {
            write!(f, "__RELPATH                    : {}\n", relpath)?;
        }
        if let Some(server) = &self.__SERVER {
            write!(f, "__SERVER                     : {}\n", server)?;
        }
        if let Some(superclass) = &self.__SUPERCLASS {
            write!(f, "__SUPERCLASS                 : {}\n", superclass)?;
        }
        Ok(())
    }
}

/// Query disk partitions via WMI (Windows only)
#[cfg(windows)]
pub fn query_disk_partitions() -> Result<Vec<Win32DiskPartition>, UFFSError> {
    // Establish a connection to WMI in the correct namespace
    let wmi_con = WMIConnection::with_namespace_path("ROOT\\CIMv2").map_err(|e| {
        UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
    })?;

    // Define the WMI query for disk partitions
    let query = "SELECT * FROM Win32_DiskPartition";

    // Execute the query and get results
    let results: Vec<Win32DiskPartition> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
