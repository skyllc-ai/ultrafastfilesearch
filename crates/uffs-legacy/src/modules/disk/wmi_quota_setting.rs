use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::{COMLibrary, WMIConnection};

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32QuotaSetting {
    /// The `Caption` property provides a short textual description of the quota
    /// setting.
    Caption: Option<String>,

    /// The `DefaultLimit` property specifies the default disk quota limit, in
    /// bytes.
    DefaultLimit: Option<i64>,

    /// The `DefaultWarningLimit` property specifies the default warning limit,
    /// in bytes.
    DefaultWarningLimit: Option<i64>,

    /// The `Description` property contains a description of the quota setting.
    Description: Option<String>,

    /// The `ExceededNotification` property indicates whether the exceeded
    /// notification is enabled.
    ExceededNotification: Option<bool>,

    /// The `SettingID` property contains a unique identifier for the quota
    /// setting.
    SettingID: Option<String>,

    /// The `State` property indicates the state of the quota setting.
    State: Option<u32>,

    /// The `VolumePath` property contains the volume path associated with the
    /// quota setting.
    VolumePath: String,

    /// The `WarningExceededNotification` property indicates whether the warning
    /// exceeded notification is enabled.
    WarningExceededNotification: Option<bool>,

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
impl fmt::Display for Win32QuotaSetting {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(caption) = &self.Caption {
            write!(f, "Caption                      : {}\n", caption)?;
        }
        if let Some(default_limit) = &self.DefaultLimit {
            write!(f, "DefaultLimit                 : {}\n", default_limit)?;
        }
        if let Some(default_warning_limit) = &self.DefaultWarningLimit {
            write!(
                f,
                "DefaultWarningLimit          : {}\n",
                default_warning_limit
            )?;
        }
        if let Some(description) = &self.Description {
            write!(f, "Description                  : {}\n", description)?;
        }
        if let Some(exceeded_notification) = &self.ExceededNotification {
            write!(
                f,
                "ExceededNotification         : {}\n",
                exceeded_notification
            )?;
        }
        if let Some(setting_id) = &self.SettingID {
            write!(f, "SettingID                    : {}\n", setting_id)?;
        }
        if let Some(state) = &self.State {
            write!(f, "State                        : {}\n", state)?;
        }
        write!(f, "VolumePath                   : {}\n", self.VolumePath)?;
        if let Some(warning_exceeded_notification) = &self.WarningExceededNotification {
            write!(
                f,
                "WarningExceededNotification  : {}\n",
                warning_exceeded_notification
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

/// Query quota settings via WMI (Windows only)
#[cfg(windows)]
pub fn query_quota_setting() -> Result<Vec<Win32QuotaSetting>, UFFSError> {
    // Initialize COM library
    let com_con = COMLibrary::new()
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to initialize COM: {:?}", e)))?;

    // Establish a connection to WMI in the correct namespace
    let wmi_con =
        WMIConnection::with_namespace_path("ROOT\\CIMv2", com_con.into()).map_err(|e| {
            UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
        })?;

    // Define the WMI query
    let query = "SELECT * FROM Win32_QuotaSetting";

    // Execute the query and get results
    let results: Vec<Win32QuotaSetting> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
