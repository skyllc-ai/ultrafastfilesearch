use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::WMIConnection;

use crate::modules::errors::UFFSError;

/// Represents the disk quota information for a user on a volume.
/// This struct corresponds to the `Win32_DiskQuota` WMI class.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32DiskQuota {
    /// The `DiskSpaceUsed` property indicates the amount of disk space used by
    /// the user, in bytes.
    pub DiskSpaceUsed: Option<u64>,

    /// The `Limit` property specifies the disk space limit set for the user, in
    /// bytes.
    pub Limit: Option<u64>,

    /// The `QuotaVolume` property contains the path to the logical disk
    /// associated with the quota.
    pub QuotaVolume: Option<String>,

    /// The `Status` property indicates the status of the disk quota.
    /// Possible values:
    /// - `0`: OK
    /// - `1`: Warning
    /// - `2`: Exceeded
    pub Status: Option<u32>,

    /// The `User` property contains the account name of the user associated
    /// with the quota.
    pub User: Option<String>,

    /// The `WarningLimit` property specifies the warning limit set for the
    /// user, in bytes.
    pub WarningLimit: Option<u64>,

    // Additional system properties if needed
    /// The `__CLASS` property specifies the WMI class of the object.
    pub __CLASS: Option<String>,

    /// The `__DERIVATION` property contains the inheritance hierarchy of the
    /// WMI class.
    pub __DERIVATION: Option<Vec<String>>,

    /// The `__DYNASTY` property specifies the root class in the inheritance
    /// hierarchy.
    pub __DYNASTY: Option<String>,

    /// The `__GENUS` property is an internal classification value used by WMI.
    pub __GENUS: Option<i32>,

    /// The `__NAMESPACE` property specifies the WMI namespace where the object
    /// resides.
    pub __NAMESPACE: Option<String>,

    /// The `__PATH` property contains the full WMI path to the object.
    pub __PATH: Option<String>,

    /// The `__PROPERTY_COUNT` property indicates the number of properties in
    /// the object.
    pub __PROPERTY_COUNT: Option<i32>,

    /// The `__RELPATH` property specifies the relative path to the object
    /// within the WMI namespace.
    pub __RELPATH: Option<String>,

    /// The `__SERVER` property specifies the name of the server where the WMI
    /// object resides.
    pub __SERVER: Option<String>,

    /// The `__SUPERCLASS` property contains the immediate superclass of the WMI
    /// class.
    pub __SUPERCLASS: Option<String>,
}

// Implement Display for a formatted output
impl fmt::Display for Win32DiskQuota {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(disk_space_used) = &self.DiskSpaceUsed {
            write!(f, "DiskSpaceUsed                : {}\n", disk_space_used)?;
        }
        if let Some(limit) = &self.Limit {
            write!(f, "Limit                        : {}\n", limit)?;
        }
        if let Some(quota_volume) = &self.QuotaVolume {
            write!(f, "QuotaVolume                  : {}\n", quota_volume)?;
        }
        if let Some(status) = &self.Status {
            write!(f, "Status                       : {}\n", status)?;
        }
        if let Some(user) = &self.User {
            write!(f, "User                         : {}\n", user)?;
        }
        if let Some(warning_limit) = &self.WarningLimit {
            write!(f, "WarningLimit                 : {}\n", warning_limit)?;
        }
        // Include system properties if necessary
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

/// Queries the `Win32_DiskQuota` WMI class to retrieve disk quota information.
///
/// # Returns
///
/// A `Result` containing a vector of `Win32DiskQuota` instances if successful,
/// or a `UFFSError` if an error occurs.
///
/// # Errors
///
/// - `UFFSError::WMIQueryFailed`: If initializing the COM library, connecting
///   to WMI, or executing the WMI query fails.
/// - `UFFSError::EmptyDriveInfo`: If the query returns no results.
/// Query disk quota information via WMI (Windows only)
#[cfg(windows)]
pub fn query_disk_quota() -> Result<Vec<Win32DiskQuota>, UFFSError> {
    // Establish a connection to WMI in the ROOT\CIMV2 namespace
    let wmi_con = WMIConnection::with_namespace_path("ROOT\\CIMV2").map_err(|e| {
        UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
    })?;

    // Define the WMI query to select all properties from Win32_DiskQuota
    let query = "SELECT * FROM Win32_DiskQuota";

    // Execute the query and collect the results
    let results: Vec<Win32DiskQuota> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if the results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the retrieved disk quota information
    Ok(results)
}
