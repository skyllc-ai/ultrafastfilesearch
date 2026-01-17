use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::{COMLibrary, WMIConnection};

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32ShadowCopy {
    /// The `Caption` property provides a short textual description of the
    /// shadow copy.
    Caption: Option<String>,

    /// The `ClientAccessible` property indicates whether the shadow copy is
    /// accessible by clients.
    ClientAccessible: Option<bool>,

    /// The `Count` property represents the count of shadow copies.
    Count: Option<u32>,

    /// The `Description` property contains a description of the shadow copy.
    Description: Option<String>,

    /// The `DeviceObject` property specifies the device object path.
    DeviceObject: Option<String>,

    /// The `Differential` property indicates if the shadow copy is
    /// differential.
    Differential: Option<bool>,

    /// The `ExposedLocally` property specifies if the shadow copy is exposed
    /// locally.
    ExposedLocally: Option<bool>,

    /// The `ExposedName` property contains the exposed name for the shadow
    /// copy.
    ExposedName: Option<String>,

    /// The `ExposedPath` property contains the exposed path for the shadow
    /// copy.
    ExposedPath: Option<String>,

    /// The `ExposedRemotely` property indicates if the shadow copy is exposed
    /// remotely.
    ExposedRemotely: Option<bool>,

    /// The `HardwareAssisted` property indicates if the shadow copy is
    /// hardware-assisted.
    HardwareAssisted: Option<bool>,

    /// The `ID` property contains the unique identifier for the shadow copy.
    ID: Option<String>,

    /// The `Imported` property indicates if the shadow copy was imported.
    Imported: Option<bool>,

    /// The `InstallDate` property specifies the installation date of the shadow
    /// copy.
    InstallDate: Option<String>,

    /// The `Name` property contains the name of the shadow copy.
    Name: Option<String>,

    /// The `NoAutoRelease` property indicates if the shadow copy is not
    /// automatically released.
    NoAutoRelease: Option<bool>,

    /// The `NotSurfaced` property indicates if the shadow copy is not surfaced.
    NotSurfaced: Option<bool>,

    /// The `NoWriters` property specifies if there are no writers for the
    /// shadow copy.
    NoWriters: Option<bool>,

    /// The `OriginatingMachine` property contains the originating machine name.
    OriginatingMachine: Option<String>,

    /// The `Persistent` property specifies if the shadow copy is persistent.
    Persistent: Option<bool>,

    /// The `Plex` property indicates if the shadow copy is plex.
    Plex: Option<bool>,

    /// The `ProviderID` property contains the provider ID of the shadow copy.
    ProviderID: Option<String>,

    /// The `ServiceMachine` property contains the service machine name.
    ServiceMachine: Option<String>,

    /// The `SetID` property contains the set ID of the shadow copy.
    SetID: Option<String>,

    /// The `State` property indicates the state of the shadow copy.
    State: Option<u32>,

    /// The `Status` property contains the status of the shadow copy.
    Status: Option<String>,

    /// The `Transportable` property indicates if the shadow copy is
    /// transportable.
    Transportable: Option<bool>,

    /// The `VolumeName` property contains the volume name associated with the
    /// shadow copy.
    VolumeName: Option<String>,

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
impl fmt::Display for Win32ShadowCopy {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(caption) = &self.Caption {
            write!(f, "Caption               : {}\n", caption)?;
        }
        if let Some(client_accessible) = &self.ClientAccessible {
            write!(f, "ClientAccessible      : {}\n", client_accessible)?;
        }
        if let Some(count) = &self.Count {
            write!(f, "Count                 : {}\n", count)?;
        }
        if let Some(description) = &self.Description {
            write!(f, "Description           : {}\n", description)?;
        }
        if let Some(device_object) = &self.DeviceObject {
            write!(f, "DeviceObject          : {}\n", device_object)?;
        }
        if let Some(differential) = &self.Differential {
            write!(f, "Differential          : {}\n", differential)?;
        }
        if let Some(exposed_locally) = &self.ExposedLocally {
            write!(f, "ExposedLocally        : {}\n", exposed_locally)?;
        }
        if let Some(exposed_name) = &self.ExposedName {
            write!(f, "ExposedName           : {}\n", exposed_name)?;
        }
        if let Some(exposed_path) = &self.ExposedPath {
            write!(f, "ExposedPath           : {}\n", exposed_path)?;
        }
        if let Some(exposed_remotely) = &self.ExposedRemotely {
            write!(f, "ExposedRemotely       : {}\n", exposed_remotely)?;
        }
        if let Some(hardware_assisted) = &self.HardwareAssisted {
            write!(f, "HardwareAssisted      : {}\n", hardware_assisted)?;
        }
        if let Some(id) = &self.ID {
            write!(f, "ID                    : {}\n", id)?;
        }
        if let Some(imported) = &self.Imported {
            write!(f, "Imported              : {}\n", imported)?;
        }
        if let Some(install_date) = &self.InstallDate {
            write!(f, "InstallDate           : {}\n", install_date)?;
        }
        if let Some(name) = &self.Name {
            write!(f, "Name                  : {}\n", name)?;
        }
        if let Some(no_auto_release) = &self.NoAutoRelease {
            write!(f, "NoAutoRelease         : {}\n", no_auto_release)?;
        }
        if let Some(not_surfaced) = &self.NotSurfaced {
            write!(f, "NotSurfaced           : {}\n", not_surfaced)?;
        }
        if let Some(no_writers) = &self.NoWriters {
            write!(f, "NoWriters             : {}\n", no_writers)?;
        }
        if let Some(originating_machine) = &self.OriginatingMachine {
            write!(f, "OriginatingMachine    : {}\n", originating_machine)?;
        }
        if let Some(persistent) = &self.Persistent {
            write!(f, "Persistent            : {}\n", persistent)?;
        }
        if let Some(plex) = &self.Plex {
            write!(f, "Plex                  : {}\n", plex)?;
        }
        if let Some(provider_id) = &self.ProviderID {
            write!(f, "ProviderID            : {}\n", provider_id)?;
        }
        if let Some(service_machine) = &self.ServiceMachine {
            write!(f, "ServiceMachine        : {}\n", service_machine)?;
        }
        if let Some(set_id) = &self.SetID {
            write!(f, "SetID                 : {}\n", set_id)?;
        }
        if let Some(state) = &self.State {
            write!(f, "State                 : {}\n", state)?;
        }
        if let Some(status) = &self.Status {
            write!(f, "Status                : {}\n", status)?;
        }
        if let Some(transportable) = &self.Transportable {
            write!(f, "Transportable         : {}\n", transportable)?;
        }
        if let Some(volume_name) = &self.VolumeName {
            write!(f, "VolumeName            : {}\n", volume_name)?;
        }
        if let Some(class) = &self.__CLASS {
            write!(f, "__CLASS               : {}\n", class)?;
        }
        if let Some(derivation) = &self.__DERIVATION {
            write!(f, "__DERIVATION          : {:?}\n", derivation)?;
        }
        if let Some(dynasty) = &self.__DYNASTY {
            write!(f, "__DYNASTY             : {}\n", dynasty)?;
        }
        if let Some(genus) = &self.__GENUS {
            write!(f, "__GENUS               : {}\n", genus)?;
        }
        if let Some(namespace) = &self.__NAMESPACE {
            write!(f, "__NAMESPACE           : {}\n", namespace)?;
        }
        if let Some(path) = &self.__PATH {
            write!(f, "__PATH                : {}\n", path)?;
        }
        if let Some(property_count) = &self.__PROPERTY_COUNT {
            write!(f, "__PROPERTY_COUNT      : {}\n", property_count)?;
        }
        if let Some(relpath) = &self.__RELPATH {
            write!(f, "__RELPATH             : {}\n", relpath)?;
        }
        if let Some(server) = &self.__SERVER {
            write!(f, "__SERVER              : {}\n", server)?;
        }
        if let Some(superclass) = &self.__SUPERCLASS {
            write!(f, "__SUPERCLASS          : {}\n", superclass)?;
        }
        Ok(())
    }
}

/// Query shadow copies via WMI (Windows only)
#[cfg(windows)]
pub fn query_shadow_copy() -> Result<Vec<Win32ShadowCopy>, UFFSError> {
    // Initialize COM library
    let com_con = COMLibrary::new()
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to initialize COM: {:?}", e)))?;

    // Establish a connection to WMI in the correct namespace
    let wmi_con =
        WMIConnection::with_namespace_path("ROOT\\CIMv2", com_con.into()).map_err(|e| {
            UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
        })?;

    // Define the WMI query
    let query = "SELECT * FROM Win32_ShadowCopy";

    // Execute the query and get results
    let results: Vec<Win32ShadowCopy> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
