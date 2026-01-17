use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::{COMLibrary, WMIConnection};

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32EncryptableVolume {
    /// The `ConversionStatus` property specifies the status of the volume
    /// encryption process.
    ConversionStatus: Option<u32>,

    /// The `DeviceID` property contains the unique identifier for the
    /// encryptable volume.
    DeviceID: String,

    /// The `DriveLetter` property specifies the drive letter assigned to the
    /// volume, if applicable.
    DriveLetter: Option<String>,

    /// The `EncryptionMethod` property indicates the encryption method used on
    /// the volume.
    EncryptionMethod: Option<u32>,

    /// The `IsVolumeInitializedForProtection` property specifies whether the
    /// volume is initialized for protection.
    IsVolumeInitializedForProtection: Option<bool>,

    /// The `PersistentVolumeID` property contains the persistent identifier for
    /// the volume.
    PersistentVolumeID: Option<String>,

    /// The `ProtectionStatus` property indicates the protection status of the
    /// volume.
    ProtectionStatus: Option<u32>,

    /// The `VolumeType` property specifies the type of the volume.
    VolumeType: Option<u32>,

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
impl fmt::Display for Win32EncryptableVolume {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(conversion_status) = &self.ConversionStatus {
            write!(f, "ConversionStatus             : {}\n", conversion_status)?;
        }
        write!(f, "DeviceID                     : {}\n", self.DeviceID)?;
        if let Some(drive_letter) = &self.DriveLetter {
            write!(f, "DriveLetter                  : {}\n", drive_letter)?;
        }
        if let Some(encryption_method) = &self.EncryptionMethod {
            write!(f, "EncryptionMethod             : {}\n", encryption_method)?;
        }
        if let Some(is_volume_initialized) = &self.IsVolumeInitializedForProtection {
            write!(
                f,
                "IsVolumeInitializedForProtection : {}\n",
                is_volume_initialized
            )?;
        }
        if let Some(persistent_volume_id) = &self.PersistentVolumeID {
            write!(
                f,
                "PersistentVolumeID           : {}\n",
                persistent_volume_id
            )?;
        }
        if let Some(protection_status) = &self.ProtectionStatus {
            write!(f, "ProtectionStatus             : {}\n", protection_status)?;
        }
        if let Some(volume_type) = &self.VolumeType {
            write!(f, "VolumeType                   : {}\n", volume_type)?;
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

/// Query encryptable volumes via WMI (Windows only)
#[cfg(windows)]
pub fn query_encryptable_volumes() -> Result<Vec<Win32EncryptableVolume>, UFFSError> {
    // Initialize COM library
    let com_con = COMLibrary::new()
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to initialize COM: {:?}", e)))?;

    // Establish a connection to WMI in the correct namespace
    let wmi_con = WMIConnection::with_namespace_path(
        "ROOT\\CIMv2\\Security\\MicrosoftVolumeEncryption",
        com_con.into(),
    )
    .map_err(|e| {
        UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
    })?;

    // Define the WMI query for encryptable volumes
    let query = "SELECT * FROM Win32_EncryptableVolume";

    // Execute the query and get results
    let results: Vec<Win32EncryptableVolume> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
