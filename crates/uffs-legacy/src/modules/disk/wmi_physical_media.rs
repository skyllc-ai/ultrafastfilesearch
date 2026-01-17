use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::WMIConnection;

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32PhysicalMedia {
    /// The `Capacity` property specifies the capacity of the physical media in
    /// bytes.
    Capacity: Option<u64>,

    /// The `Caption` property provides a short textual description of the
    /// physical media.
    Caption: Option<String>,

    /// The `CleanerMedia` property indicates if the physical media is a cleaner
    /// media.
    CleanerMedia: Option<bool>,

    /// The `CreationClassName` property specifies the name of the class or
    /// subclass used to create the media.
    CreationClassName: Option<String>,

    /// The `Description` property contains a description of the physical media.
    Description: Option<String>,

    /// The `HotSwappable` property indicates whether the media is hot
    /// swappable.
    HotSwappable: Option<bool>,

    /// The `InstallDate` property specifies the installation date of the
    /// physical media.
    InstallDate: Option<String>,

    /// The `Manufacturer` property specifies the manufacturer of the physical
    /// media.
    Manufacturer: Option<String>,

    /// The `MediaDescription` property contains a description of the media
    /// type.
    MediaDescription: Option<String>,

    /// The `MediaType` property specifies the type of the physical media (e.g.,
    /// "Fixed Disk").
    MediaType: Option<String>,

    /// The `Model` property contains the model name of the physical media.
    Model: Option<String>,

    /// The `Name` property specifies the name of the physical media.
    Name: Option<String>,

    /// The `OtherIdentifyingInfo` property provides additional identifying
    /// information about the physical media.
    OtherIdentifyingInfo: Option<String>,

    /// The `PartNumber` property specifies the part number for the physical
    /// media.
    PartNumber: Option<String>,

    /// The `PoweredOn` property indicates whether the physical media is
    /// currently powered on.
    PoweredOn: Option<bool>,

    /// The `Removable` property specifies whether the physical media is
    /// removable.
    Removable: Option<bool>,

    /// The `Replaceable` property indicates whether the physical media is
    /// replaceable.
    Replaceable: Option<bool>,

    /// The `SerialNumber` property contains the serial number of the physical
    /// media.
    SerialNumber: Option<String>,

    /// The `SKU` property specifies the stock-keeping unit for the physical
    /// media.
    SKU: Option<String>,

    /// The `Status` property contains the current status of the physical media.
    Status: Option<String>,

    /// The `Tag` property contains a unique identifier for the physical media,
    /// often corresponding to the device path.
    Tag: Option<String>,

    /// The `Version` property specifies the version of the physical media.
    Version: Option<String>,

    /// The `WriteProtectOn` property indicates whether the physical media is
    /// write-protected.
    WriteProtectOn: Option<bool>,

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
impl fmt::Display for Win32PhysicalMedia {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(caption) = &self.Caption {
            write!(f, "Caption                      : {}\n", caption)?;
        }
        if let Some(description) = &self.Description {
            write!(f, "Description                  : {}\n", description)?;
        }
        if let Some(serial_number) = &self.SerialNumber {
            write!(f, "SerialNumber                 : {}\n", serial_number)?;
        }
        if let Some(media_type) = &self.MediaType {
            write!(f, "MediaType                    : {}\n", media_type)?;
        }
        if let Some(capacity) = &self.Capacity {
            write!(f, "Capacity                     : {}\n", capacity)?;
        }
        if let Some(tag) = &self.Tag {
            write!(f, "Tag                          : {}\n", tag)?;
        }
        if let Some(manufacturer) = &self.Manufacturer {
            write!(f, "Manufacturer                 : {}\n", manufacturer)?;
        }
        if let Some(model) = &self.Model {
            write!(f, "Model                        : {}\n", model)?;
        }
        if let Some(part_number) = &self.PartNumber {
            write!(f, "PartNumber                   : {}\n", part_number)?;
        }
        if let Some(status) = &self.Status {
            write!(f, "Status                       : {}\n", status)?;
        }
        if let Some(powered_on) = &self.PoweredOn {
            write!(f, "PoweredOn                    : {}\n", powered_on)?;
        }
        if let Some(removable) = &self.Removable {
            write!(f, "Removable                    : {}\n", removable)?;
        }
        if let Some(replaceable) = &self.Replaceable {
            write!(f, "Replaceable                  : {}\n", replaceable)?;
        }
        if let Some(write_protect_on) = &self.WriteProtectOn {
            write!(f, "WriteProtectOn               : {}\n", write_protect_on)?;
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

/// Query physical media information via WMI (Windows only)
#[cfg(windows)]
pub fn query_physical_media() -> Result<Vec<Win32PhysicalMedia>, UFFSError> {
    // Establish a connection to WMI in the correct namespace
    let wmi_con = WMIConnection::with_namespace_path("ROOT\\CIMv2").map_err(|e| {
        UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
    })?;

    // Define the WMI query for physical media
    let query = "SELECT * FROM Win32_PhysicalMedia";

    // Execute the query and get results
    let results: Vec<Win32PhysicalMedia> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
