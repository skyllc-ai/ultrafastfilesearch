use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::WMIConnection;

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MSFTPartition {
    /// The `AccessPaths` property contains the access paths to the partition.
    AccessPaths: Option<Vec<String>>,

    /// The `DiskId` property contains the ID of the disk on which the partition
    /// resides.
    DiskId: Option<String>,

    /// The `DiskNumber` property indicates the disk number of the partition.
    DiskNumber: Option<u32>, // This is a UInt32 in WMI

    /// The `DriveLetter` property contains the drive letter assigned to the
    /// partition.
    DriveLetter: Option<String>, // DriveLetter is typically a single-character string in WMI

    /// The `GptType` property contains the GPT partition type GUID.
    GptType: Option<String>,

    /// The `Guid` property contains the GUID of the partition.
    Guid: Option<String>,

    /// The `IsActive` property indicates whether the partition is active.
    IsActive: Option<bool>, // Boolean in WMI

    /// The `IsBoot` property indicates whether the partition is a boot
    /// partition.
    IsBoot: Option<bool>, // Boolean in WMI

    /// The `IsDAX` property indicates whether the partition is a DAX partition.
    IsDAX: Option<bool>, // Missing type, should be Boolean

    /// The `IsHidden` property indicates whether the partition is hidden.
    IsHidden: Option<bool>, // Boolean in WMI

    /// The `IsOffline` property indicates whether the partition is offline.
    IsOffline: Option<bool>, // Boolean in WMI

    /// The `IsReadOnly` property indicates whether the partition is read-only.
    IsReadOnly: Option<bool>, // Missing type, should be Boolean

    /// The `IsShadowCopy` property indicates whether the partition is a shadow
    /// copy.
    IsShadowCopy: Option<bool>, // Missing type, should be Boolean

    /// The `IsSystem` property indicates whether the partition is a system
    /// partition.
    IsSystem: Option<bool>, // Boolean in WMI

    /// The `MbrType` property contains the MBR partition type.
    MbrType: Option<u32>, // Missing type, assumed to be String

    /// The `NoDefaultDriveLetter` property indicates whether the partition has
    /// no default drive letter.
    NoDefaultDriveLetter: Option<bool>, // Missing type, should be Boolean

    /// The `ObjectId` property contains the unique identifier for the partition
    /// object.
    ObjectId: Option<String>,

    /// The `Offset` property indicates the offset of the partition in bytes.
    Offset: Option<u64>, // UInt64 in WMI

    /// The `OperationalStatus` property indicates the operational status of the
    /// partition.
    OperationalStatus: Option<u16>, // UInt16 in WMI

    /// The `PartitionNumber` property indicates the partition number.
    PartitionNumber: Option<u32>, // UInt32 in WMI

    /// The `PassThroughClass` property indicates a pass-through class.
    PassThroughClass: Option<String>, // Missing type, assumed to be String

    /// The `PassThroughIds` property contains pass-through IDs.
    PassThroughIds: Option<String>, // Missing type, assumed to be String

    /// The `PassThroughNamespace` property contains a pass-through namespace.
    PassThroughNamespace: Option<String>, // Missing type, assumed to be String

    /// The `PassThroughServer` property contains a pass-through server.
    PassThroughServer: Option<String>, // Missing type, assumed to be String

    /// The `Size` property contains the size of the partition in bytes.
    Size: Option<u64>, // UInt64 in WMI

    /// The `TransitionState` property indicates the transition state of the
    /// partition.
    TransitionState: Option<u16>, // UInt16 in WMI

    /// The `UniqueId` property contains the unique identifier for the
    /// partition.
    UniqueId: Option<String>,

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
impl fmt::Display for MSFTPartition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(access_paths) = &self.AccessPaths {
            write!(f, "AccessPaths             : {:?}\n", access_paths)?;
        }
        if let Some(disk_id) = &self.DiskId {
            write!(f, "DiskId                  : {}\n", disk_id)?;
        }
        if let Some(disk_number) = &self.DiskNumber {
            write!(f, "DiskNumber              : {}\n", disk_number)?;
        }
        if let Some(drive_letter) = &self.DriveLetter {
            write!(f, "DriveLetter             : {}\n", drive_letter)?;
        }
        if let Some(gpt_type) = &self.GptType {
            write!(f, "GptType                 : {}\n", gpt_type)?;
        }
        if let Some(guid) = &self.Guid {
            write!(f, "Guid                    : {}\n", guid)?;
        }
        if let Some(is_active) = &self.IsActive {
            write!(f, "IsActive                : {}\n", is_active)?;
        }
        if let Some(is_boot) = &self.IsBoot {
            write!(f, "IsBoot                  : {}\n", is_boot)?;
        }
        if let Some(is_dax) = &self.IsDAX {
            write!(f, "IsDAX                   : {}\n", is_dax)?;
        }
        if let Some(is_hidden) = &self.IsHidden {
            write!(f, "IsHidden                : {}\n", is_hidden)?;
        }
        if let Some(is_offline) = &self.IsOffline {
            write!(f, "IsOffline               : {}\n", is_offline)?;
        }
        if let Some(is_read_only) = &self.IsReadOnly {
            write!(f, "IsReadOnly              : {}\n", is_read_only)?;
        }
        if let Some(is_shadow_copy) = &self.IsShadowCopy {
            write!(f, "IsShadowCopy            : {}\n", is_shadow_copy)?;
        }
        if let Some(is_system) = &self.IsSystem {
            write!(f, "IsSystem                : {}\n", is_system)?;
        }
        if let Some(mbr_type) = &self.MbrType {
            write!(f, "MbrType                 : {}\n", mbr_type)?;
        }
        if let Some(no_default_drive_letter) = &self.NoDefaultDriveLetter {
            write!(f, "NoDefaultDriveLetter    : {}\n", no_default_drive_letter)?;
        }
        if let Some(object_id) = &self.ObjectId {
            write!(f, "ObjectId                : {}\n", object_id)?;
        }
        if let Some(offset) = &self.Offset {
            write!(f, "Offset                  : {}\n", offset)?;
        }
        if let Some(operational_status) = &self.OperationalStatus {
            write!(f, "OperationalStatus       : {}\n", operational_status)?;
        }
        if let Some(partition_number) = &self.PartitionNumber {
            write!(f, "PartitionNumber         : {}\n", partition_number)?;
        }
        if let Some(size) = &self.Size {
            write!(f, "Size                    : {}\n", size)?;
        }
        if let Some(transition_state) = &self.TransitionState {
            write!(f, "TransitionState         : {}\n", transition_state)?;
        }
        if let Some(unique_id) = &self.UniqueId {
            write!(f, "UniqueId                : {}\n", unique_id)?;
        }
        if let Some(class) = &self.__CLASS {
            write!(f, "__CLASS                 : {}\n", class)?;
        }
        if let Some(derivation) = &self.__DERIVATION {
            write!(f, "__DERIVATION            : {:?}\n", derivation)?;
        }
        if let Some(dynasty) = &self.__DYNASTY {
            write!(f, "__DYNASTY               : {}\n", dynasty)?;
        }
        if let Some(genus) = &self.__GENUS {
            write!(f, "__GENUS                 : {}\n", genus)?;
        }
        if let Some(namespace) = &self.__NAMESPACE {
            write!(f, "__NAMESPACE             : {}\n", namespace)?;
        }
        if let Some(path) = &self.__PATH {
            write!(f, "__PATH                  : {}\n", path)?;
        }
        if let Some(property_count) = &self.__PROPERTY_COUNT {
            write!(f, "__PROPERTY_COUNT        : {}\n", property_count)?;
        }
        if let Some(server) = &self.__SERVER {
            write!(f, "__SERVER                : {}\n", server)?;
        }
        if let Some(superclass) = &self.__SUPERCLASS {
            write!(f, "__SUPERCLASS            : {}\n", superclass)?;
        }
        Ok(())
    }
}

/// Query MSFT partitions via WMI (Windows only)
#[cfg(windows)]
pub fn query_msft_partition() -> Result<Vec<MSFTPartition>, UFFSError> {
    // Establish a connection to WMI in the correct namespace
    let wmi_con =
        WMIConnection::with_namespace_path("ROOT\\Microsoft\\Windows\\Storage").map_err(|e| {
            UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
        })?;

    // Define the WMI query
    let query = "SELECT * FROM MSFT_Partition";

    // Execute the query and get results
    let results: Vec<MSFTPartition> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
