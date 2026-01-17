use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::{COMLibrary, WMIConnection};

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32DiskDrive {
    /// The `Availability` property indicates the availability and status of the
    /// disk drive.
    Availability: Option<u16>,

    /// The `BytesPerSector` property indicates the number of bytes per sector.
    BytesPerSector: Option<u32>,

    /// The `Capabilities` property lists the disk's capabilities, such as
    /// supporting random access or writing.
    Capabilities: Option<Vec<u16>>,

    /// The `CapabilityDescriptions` property contains descriptions of each
    /// capability.
    CapabilityDescriptions: Option<Vec<String>>,

    /// The `Caption` property provides a short textual description of the disk
    /// drive.
    Caption: Option<String>,

    /// The `CompressionMethod` property specifies the compression method used
    /// on the disk, if any.
    CompressionMethod: Option<String>,

    /// The `ConfigManagerErrorCode` property contains an error code, if
    /// applicable.
    ConfigManagerErrorCode: Option<u32>,

    /// The `ConfigManagerUserConfig` property indicates whether the disk is
    /// user-configurable.
    ConfigManagerUserConfig: Option<bool>,

    /// The `CreationClassName` property contains the name of the class or
    /// subclass used to create the disk drive.
    CreationClassName: Option<String>,

    /// The `Description` property contains a description of the disk drive.
    Description: Option<String>,

    /// The `DeviceID` property contains the unique identifier for the disk
    /// drive.
    DeviceID: String,

    /// The `FirmwareRevision` property specifies the firmware revision of the
    /// disk drive.
    FirmwareRevision: Option<String>,

    /// The `Index` property contains the index of the disk drive on the system.
    Index: Option<u32>,

    /// The `InstallDate` property specifies the date the disk drive was
    /// installed.
    InstallDate: Option<String>,

    /// The `InterfaceType` property contains the type of interface used by the
    /// disk drive (e.g., SCSI, IDE).
    InterfaceType: Option<String>,

    /// The `LastErrorCode` property contains the last error code encountered.
    LastErrorCode: Option<u32>,

    /// The `Manufacturer` property specifies the manufacturer of the disk
    /// drive.
    Manufacturer: Option<String>,

    /// The `MediaLoaded` property indicates whether a disk is loaded in the
    /// drive.
    MediaLoaded: Option<bool>,

    /// The `MediaType` property describes the type of media in the drive (e.g.,
    /// "Fixed hard disk media").
    MediaType: Option<String>,

    /// The `Model` property contains the model name of the disk drive.
    Model: Option<String>,

    /// The `Name` property specifies the name of the disk drive.
    Name: Option<String>,

    /// The `Partitions` property contains the number of partitions on the disk.
    Partitions: Option<u32>,

    /// The `PNPDeviceID` property specifies the Plug and Play device
    /// identifier.
    PNPDeviceID: Option<String>,

    /// The `SCSIBus` property contains the SCSI bus number that the disk drive
    /// is connected to.
    SCSIBus: Option<u32>,

    /// The `SCSILogicalUnit` property specifies the SCSI logical unit number of
    /// the disk drive.
    SCSILogicalUnit: Option<u16>,

    /// The `SCSIPort` property contains the SCSI port number of the disk drive.
    SCSIPort: Option<u16>,

    /// The `SCSITargetId` property specifies the SCSI target ID of the disk
    /// drive.
    SCSITargetId: Option<u16>,

    /// The `SectorsPerTrack` property contains the number of sectors per track
    /// on the disk.
    SectorsPerTrack: Option<u32>,

    /// The `SerialNumber` property contains the serial number of the disk
    /// drive.
    SerialNumber: Option<String>,

    /// The `Size` property specifies the total size of the disk drive, in
    /// bytes.
    Size: Option<u64>,

    /// The `Status` property provides the current status of the disk drive.
    Status: Option<String>,

    /// The `SystemCreationClassName` property specifies the name of the class
    /// used to create the system containing the disk drive.
    SystemCreationClassName: Option<String>,

    /// The `SystemName` property contains the name of the system where the disk
    /// drive is located.
    SystemName: Option<String>,

    /// The `TotalCylinders` property contains the total number of cylinders on
    /// the disk drive.
    TotalCylinders: Option<u64>,

    /// The `TotalHeads` property specifies the total number of heads on the
    /// disk drive.
    TotalHeads: Option<u32>,

    /// The `TotalSectors` property contains the total number of sectors on the
    /// disk drive.
    TotalSectors: Option<u64>,

    /// The `TotalTracks` property contains the total number of tracks on the
    /// disk drive.
    TotalTracks: Option<u64>,

    /// The `TracksPerCylinder` property specifies the number of tracks per
    /// cylinder on the disk drive.
    TracksPerCylinder: Option<u32>,

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
impl fmt::Display for Win32DiskDrive {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(caption) = &self.Caption {
            write!(f, "Caption                      : {}\n", caption)?;
        }
        if let Some(description) = &self.Description {
            write!(f, "Description                  : {}\n", description)?;
        }
        if let Some(model) = &self.Model {
            write!(f, "Model                        : {}\n", model)?;
        }
        write!(f, "DeviceID                     : {}\n", self.DeviceID)?;
        if let Some(firmware_revision) = &self.FirmwareRevision {
            write!(f, "FirmwareRevision             : {}\n", firmware_revision)?;
        }
        if let Some(index) = &self.Index {
            write!(f, "Index                        : {}\n", index)?;
        }
        if let Some(interface_type) = &self.InterfaceType {
            write!(f, "InterfaceType                : {}\n", interface_type)?;
        }
        if let Some(manufacturer) = &self.Manufacturer {
            write!(f, "Manufacturer                 : {}\n", manufacturer)?;
        }
        if let Some(media_type) = &self.MediaType {
            write!(f, "MediaType                    : {}\n", media_type)?;
        }
        if let Some(serial_number) = &self.SerialNumber {
            write!(f, "SerialNumber                 : {}\n", serial_number)?;
        }
        if let Some(size) = &self.Size {
            write!(f, "Size                         : {}\n", size)?;
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
        if let Some(total_cylinders) = &self.TotalCylinders {
            write!(f, "TotalCylinders               : {}\n", total_cylinders)?;
        }
        if let Some(total_heads) = &self.TotalHeads {
            write!(f, "TotalHeads                   : {}\n", total_heads)?;
        }
        if let Some(total_sectors) = &self.TotalSectors {
            write!(f, "TotalSectors                 : {}\n", total_sectors)?;
        }
        if let Some(total_tracks) = &self.TotalTracks {
            write!(f, "TotalTracks                  : {}\n", total_tracks)?;
        }
        if let Some(tracks_per_cylinder) = &self.TracksPerCylinder {
            write!(
                f,
                "TracksPerCylinder            : {}\n",
                tracks_per_cylinder
            )?;
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

/// Query disk drives via WMI (Windows only)
#[cfg(windows)]
pub fn query_disk_drives() -> Result<Vec<Win32DiskDrive>, UFFSError> {
    // Initialize COM library
    let com_con = COMLibrary::new()
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to initialize COM: {:?}", e)))?;

    // Establish a connection to WMI in the correct namespace
    let wmi_con =
        WMIConnection::with_namespace_path("ROOT\\CIMv2", com_con.into()).map_err(|e| {
            UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
        })?;

    // Define the WMI query for disk drives
    let query = "SELECT * FROM Win32_DiskDrive";

    // Execute the query and get results
    let results: Vec<Win32DiskDrive> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
