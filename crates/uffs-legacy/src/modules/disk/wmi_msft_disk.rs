use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::{COMLibrary, WMIConnection};

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MSFTDisk {
    /// The `AdapterSerialNumber` property contains the adapter serial number
    /// for the disk.
    AdapterSerialNumber: Option<String>,

    /// The `AllocatedSize` property indicates the size allocated to the disk in
    /// bytes.
    AllocatedSize: Option<u64>,

    /// The `BootFromDisk` property specifies if the disk is the boot disk.
    BootFromDisk: Option<bool>,

    /// The `BusType` property indicates the bus type for the disk (e.g., SCSI,
    /// IDE).
    BusType: Option<u16>,

    /// The `FirmwareVersion` property contains the firmware version of the
    /// disk.
    FirmwareVersion: Option<String>,

    /// The `FriendlyName` property provides a user-friendly name for the disk.
    FriendlyName: Option<String>,

    /// The `Guid` property contains the globally unique identifier (GUID) for
    /// the disk.
    Guid: Option<String>,

    /// The `HealthStatus` property indicates the health status of the disk.
    HealthStatus: Option<u16>,

    /// The `IsBoot` property specifies if the disk is a boot disk.
    IsBoot: Option<bool>,

    /// The `IsClustered` property indicates whether the disk is part of a
    /// cluster.
    IsClustered: Option<bool>,

    /// The `IsHighlyAvailable` property specifies whether the disk is highly
    /// available.
    IsHighlyAvailable: Option<bool>,

    /// The `IsOffline` property indicates whether the disk is offline.
    IsOffline: Option<bool>,

    /// The `IsReadOnly` property specifies whether the disk is read-only.
    IsReadOnly: Option<bool>,

    /// The `IsScaleOut` property indicates whether the disk is part of a
    /// scale-out environment.
    IsScaleOut: Option<bool>,

    /// The `IsSystem` property indicates whether the disk is a system disk.
    IsSystem: Option<bool>,

    /// The `LargestFreeExtent` property specifies the size of the largest free
    /// extent on the disk in bytes.
    LargestFreeExtent: Option<u64>,

    /// The `Location` property specifies the physical location of the disk.
    Location: Option<String>,

    /// The `LogicalSectorSize` property indicates the logical sector size of
    /// the disk in bytes.
    LogicalSectorSize: Option<u32>,

    /// The `Manufacturer` property specifies the manufacturer of the disk.
    Manufacturer: Option<String>,

    /// The `Model` property contains the model name of the disk.
    Model: Option<String>,

    /// The `Number` property specifies the disk number.
    Number: Option<u32>,

    /// The `NumberOfPartitions` property indicates the number of partitions on
    /// the disk.
    NumberOfPartitions: Option<u32>,

    /// The `ObjectId` property contains the unique object identifier for the
    /// disk.
    ObjectId: Option<String>,

    /// The `OfflineReason` property specifies the reason why the disk is
    /// offline, if applicable.
    OfflineReason: Option<u16>,

    /// The `OperationalStatus` property lists the operational status of the
    /// disk.
    OperationalStatus: Option<Vec<u16>>,

    /// The `PartitionStyle` property indicates the partition style of the disk
    /// (e.g., MBR, GPT).
    PartitionStyle: Option<u16>,

    /// The `Path` property contains the path to the disk.
    Path: Option<String>,

    /// The `PhysicalSectorSize` property indicates the physical sector size of
    /// the disk in bytes.
    PhysicalSectorSize: Option<u32>,

    /// The `ProvisioningType` property specifies the provisioning type of the
    /// disk (e.g., Thin, Fixed).
    ProvisioningType: Option<u16>,

    /// The `SerialNumber` property contains the serial number of the disk.
    SerialNumber: Option<String>,

    /// The `Size` property specifies the total size of the disk in bytes.
    Size: Option<u64>,

    /// The `UniqueId` property contains the unique identifier for the disk.
    UniqueId: Option<String>,

    /// The `UniqueIdFormat` property specifies the format of the unique
    /// identifier.
    UniqueIdFormat: Option<u16>,

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
impl fmt::Display for MSFTDisk {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(friendly_name) = &self.FriendlyName {
            write!(f, "FriendlyName                 : {}\n", friendly_name)?;
        }
        if let Some(model) = &self.Model {
            write!(f, "Model                        : {}\n", model)?;
        }
        if let Some(serial_number) = &self.SerialNumber {
            write!(f, "SerialNumber                 : {}\n", serial_number)?;
        }
        if let Some(size) = &self.Size {
            write!(f, "Size                         : {}\n", size)?;
        }
        if let Some(guid) = &self.Guid {
            write!(f, "Guid                         : {}\n", guid)?;
        }
        if let Some(bus_type) = &self.BusType {
            write!(f, "BusType                      : {}\n", bus_type)?;
        }
        if let Some(firmware_version) = &self.FirmwareVersion {
            write!(f, "FirmwareVersion              : {}\n", firmware_version)?;
        }
        if let Some(health_status) = &self.HealthStatus {
            write!(f, "HealthStatus                 : {}\n", health_status)?;
        }
        if let Some(partition_style) = &self.PartitionStyle {
            write!(f, "PartitionStyle               : {}\n", partition_style)?;
        }
        if let Some(is_boot) = &self.IsBoot {
            write!(f, "IsBoot                       : {}\n", is_boot)?;
        }
        if let Some(is_offline) = &self.IsOffline {
            write!(f, "IsOffline                    : {}\n", is_offline)?;
        }
        if let Some(allocated_size) = &self.AllocatedSize {
            write!(f, "AllocatedSize                : {}\n", allocated_size)?;
        }
        if let Some(largest_free_extent) = &self.LargestFreeExtent {
            write!(
                f,
                "LargestFreeExtent            : {}\n",
                largest_free_extent
            )?;
        }
        if let Some(logical_sector_size) = &self.LogicalSectorSize {
            write!(
                f,
                "LogicalSectorSize            : {}\n",
                logical_sector_size
            )?;
        }
        if let Some(physical_sector_size) = &self.PhysicalSectorSize {
            write!(
                f,
                "PhysicalSectorSize           : {}\n",
                physical_sector_size
            )?;
        }
        if let Some(location) = &self.Location {
            write!(f, "Location                     : {}\n", location)?;
        }
        if let Some(path) = &self.Path {
            write!(f, "Path                         : {}\n", path)?;
        }
        if let Some(unique_id) = &self.UniqueId {
            write!(f, "UniqueId                     : {}\n", unique_id)?;
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

/// Query MSFT disks via WMI (Windows only)
#[cfg(windows)]
pub fn query_msft_disks() -> Result<Vec<MSFTDisk>, UFFSError> {
    // Initialize COM library
    let com_con = COMLibrary::new()
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to initialize COM: {:?}", e)))?;

    // Establish a connection to WMI in the correct namespace
    let wmi_con =
        WMIConnection::with_namespace_path("ROOT\\Microsoft\\Windows\\Storage", com_con.into())
            .map_err(|e| {
                UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
            })?;

    // Define the WMI query for MSFT disks
    let query = "SELECT * FROM MSFT_Disk";

    // Execute the query and get results
    let results: Vec<MSFTDisk> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
