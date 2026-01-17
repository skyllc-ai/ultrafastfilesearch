use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::WMIConnection;

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32MountPoint {
    /// The `Directory` property contains the WMI path to the associated
    /// directory For example: `Win32_Directory.Name="F:\\"`
    Directory: String,

    /// The `Volume` property contains the WMI path to the associated volume
    /// For example: `Win32_Volume.DeviceID="\\\\?\\Volume{GUID}\\"`
    Volume: String,

    /// The `__CLASS` property specifies the WMI class of the object.
    /// For this struct, it will be `Win32_MountPoint`.
    __CLASS: Option<String>,

    /// The `__DERIVATION` property contains the inheritance hierarchy of the
    /// WMI class. It provides the list of parent classes, starting from the
    /// immediate parent.
    __DERIVATION: Option<Vec<String>>,

    /// The `__DYNASTY` property specifies the root class in the inheritance
    /// hierarchy. For `Win32_MountPoint`, this might be `Win32_MountPoint`.
    __DYNASTY: Option<String>,

    /// The `__GENUS` property is an internal classification value used by WMI.
    /// `1` represents a WMI class definition, and `2` represents an instance.
    __GENUS: Option<i32>,

    /// The `__NAMESPACE` property specifies the WMI namespace where the object
    /// resides. For example, it could be `ROOT\\CIMv2`.
    __NAMESPACE: Option<String>,

    /// The `__PATH` property contains the full WMI path to the object.
    /// This is often a fully qualified path including server, namespace, class,
    /// and key properties.
    __PATH: Option<String>,

    /// The `__PROPERTY_COUNT` property indicates the number of properties in
    /// the object. This value represents how many properties (data points)
    /// are available in this WMI object.
    __PROPERTY_COUNT: Option<i32>,

    /// The `__RELPATH` property specifies the relative path to the object
    /// within the WMI namespace. This is often used for internal references
    /// and is a shorter version of the full WMI path.
    __RELPATH: Option<String>,

    /// The `__SERVER` property specifies the name of the server where the WMI
    /// object resides. In most cases, this will be the local machine name.
    __SERVER: Option<String>,

    /// The `__SUPERCLASS` property contains the immediate superclass of the WMI
    /// class. It shows which class `Win32_MountPoint` inherits from.
    __SUPERCLASS: Option<String>,
}

// Implement Display for a nicer output format
impl fmt::Display for Win32MountPoint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Directory        : {}\n", self.Directory)?;
        write!(f, "Volume           : {}\n", self.Volume)?;
        if let Some(class) = &self.__CLASS {
            write!(f, "__CLASS          : {}\n", class)?;
        }
        if let Some(derivation) = &self.__DERIVATION {
            write!(f, "__DERIVATION     : {:?}\n", derivation)?;
        }
        if let Some(dynasty) = &self.__DYNASTY {
            write!(f, "__DYNASTY        : {}\n", dynasty)?;
        }
        if let Some(genus) = &self.__GENUS {
            write!(f, "__GENUS          : {}\n", genus)?;
        }
        if let Some(namespace) = &self.__NAMESPACE {
            write!(f, "__NAMESPACE      : {}\n", namespace)?;
        }
        if let Some(path) = &self.__PATH {
            write!(f, "__PATH           : {}\n", path)?;
        }
        if let Some(property_count) = &self.__PROPERTY_COUNT {
            write!(f, "__PROPERTY_COUNT : {}\n", property_count)?;
        }
        if let Some(relpath) = &self.__RELPATH {
            write!(f, "__RELPATH        : {}\n", relpath)?;
        }
        if let Some(server) = &self.__SERVER {
            write!(f, "__SERVER         : {}\n", server)?;
        }
        if let Some(superclass) = &self.__SUPERCLASS {
            write!(f, "__SUPERCLASS     : {}\n", superclass)?;
        }
        Ok(())
    }
}

/// Query mount points via WMI (Windows only)
#[cfg(windows)]
pub fn query_mount_point() -> Result<Vec<Win32MountPoint>, UFFSError> {
    // Establish a connection to WMI in the correct namespace
    let wmi_con = WMIConnection::with_namespace_path("ROOT\\CIMv2").map_err(|e| {
        UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
    })?;

    // Define the WMI query
    let query = "SELECT * FROM Win32_MountPoint";

    // Execute the query and get results
    let results: Vec<Win32MountPoint> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
