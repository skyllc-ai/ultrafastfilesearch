use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::WMIConnection;

use crate::modules::errors::UFFSError;

/// Represents the defragmentation analysis of a volume.
/// This struct corresponds to the `Win32_DefragAnalysis` WMI class.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Win32DefragAnalysis {
    /// The `AverageFileSize` property indicates the average size of files on
    /// the volume, in bytes.
    pub AverageFileSize: Option<u64>,

    /// The `AverageFragmentsPerFile` property indicates the average number of
    /// fragments per file.
    pub AverageFragmentsPerFile: Option<f64>,

    /// The `AverageFreeSpacePerExtent` property indicates the average size of
    /// free space extents, in bytes.
    pub AverageFreeSpacePerExtent: Option<u64>,

    /// The `ClusterSize` property indicates the cluster size of the file
    /// system, in bytes.
    pub ClusterSize: Option<u64>,

    /// The `ExcessFolderFragments` property indicates the number of excess
    /// fragments in folders.
    pub ExcessFolderFragments: Option<u64>,

    /// The `FilePercentFragmentation` property indicates the percentage of file
    /// fragmentation.
    pub FilePercentFragmentation: Option<u32>,

    /// The `FragmentedFolders` property indicates the number of fragmented
    /// folders on the volume.
    pub FragmentedFolders: Option<u64>,

    /// The `FreeSpace` property indicates the amount of free space on the
    /// volume, in bytes.
    pub FreeSpace: Option<u64>,

    /// The `FreeSpacePercent` property indicates the percentage of free space
    /// on the volume.
    pub FreeSpacePercent: Option<u32>,

    /// The `FreeSpacePercentFragmentation` property is deprecated.
    pub FreeSpacePercentFragmentation: Option<u32>,

    /// The `LargestFreeSpaceExtent` property indicates the size of the largest
    /// contiguous free space extent, in bytes.
    pub LargestFreeSpaceExtent: Option<u64>,

    /// The `MFTPercentInUse` property indicates the percentage of the Master
    /// File Table (MFT) in use.
    pub MFTPercentInUse: Option<u32>,

    /// The `MFTRecordCount` property indicates the number of MFT records on the
    /// volume.
    pub MFTRecordCount: Option<u64>,

    /// The `PageFileSize` property is deprecated.
    pub PageFileSize: Option<u64>,

    /// The `TotalExcessFragments` property indicates the total number of excess
    /// fragments on the volume.
    pub TotalExcessFragments: Option<u64>,

    /// The `TotalFiles` property indicates the total number of files on the
    /// volume.
    pub TotalFiles: Option<u64>,

    /// The `TotalFolders` property indicates the total number of folders on the
    /// volume.
    pub TotalFolders: Option<u64>,

    /// The `TotalFragmentedFiles` property indicates the total number of
    /// fragmented files on the volume.
    pub TotalFragmentedFiles: Option<u64>,

    /// The `TotalFreeSpaceExtents` property indicates the total number of free
    /// space extents on the volume.
    pub TotalFreeSpaceExtents: Option<u64>,

    /// The `TotalMFTFragments` property indicates the total number of fragments
    /// in the MFT.
    pub TotalMFTFragments: Option<u64>,

    /// The `TotalMFTSize` property indicates the total size of the MFT, in
    /// bytes.
    pub TotalMFTSize: Option<u64>,

    /// The `TotalPageFileFragments` property is deprecated.
    pub TotalPageFileFragments: Option<u64>,

    /// The `TotalPercentFragmentation` property is deprecated.
    pub TotalPercentFragmentation: Option<u32>,

    /// The `TotalUnmovableFiles` property indicates the total number of
    /// unmovable files on the volume.
    pub TotalUnmovableFiles: Option<u64>,

    /// The `UsedSpace` property indicates the amount of used space on the
    /// volume, in bytes.
    pub UsedSpace: Option<u64>,

    /// The `VolumeName` property indicates the name of the volume.
    pub VolumeName: Option<String>,

    /// The `VolumeSize` property indicates the total size of the volume, in
    /// bytes.
    pub VolumeSize: Option<u64>,

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

// Implement Display for formatted output
impl fmt::Display for Win32DefragAnalysis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(value) = &self.AverageFileSize {
            write!(f, "AverageFileSize                 : {}\n", value)?;
        }
        if let Some(value) = &self.AverageFragmentsPerFile {
            write!(f, "AverageFragmentsPerFile          : {}\n", value)?;
        }
        if let Some(value) = &self.AverageFreeSpacePerExtent {
            write!(f, "AverageFreeSpacePerExtent        : {}\n", value)?;
        }
        if let Some(value) = &self.ClusterSize {
            write!(f, "ClusterSize                      : {}\n", value)?;
        }
        if let Some(value) = &self.ExcessFolderFragments {
            write!(f, "ExcessFolderFragments            : {}\n", value)?;
        }
        if let Some(value) = &self.FilePercentFragmentation {
            write!(f, "FilePercentFragmentation         : {}\n", value)?;
        }
        if let Some(value) = &self.FragmentedFolders {
            write!(f, "FragmentedFolders                : {}\n", value)?;
        }
        if let Some(value) = &self.FreeSpace {
            write!(f, "FreeSpace                        : {}\n", value)?;
        }
        if let Some(value) = &self.FreeSpacePercent {
            write!(f, "FreeSpacePercent                 : {}\n", value)?;
        }
        if let Some(value) = &self.FreeSpacePercentFragmentation {
            write!(f, "FreeSpacePercentFragmentation    : {}\n", value)?;
        }
        if let Some(value) = &self.LargestFreeSpaceExtent {
            write!(f, "LargestFreeSpaceExtent           : {}\n", value)?;
        }
        if let Some(value) = &self.MFTPercentInUse {
            write!(f, "MFTPercentInUse                  : {}\n", value)?;
        }
        if let Some(value) = &self.MFTRecordCount {
            write!(f, "MFTRecordCount                   : {}\n", value)?;
        }
        if let Some(value) = &self.PageFileSize {
            write!(f, "PageFileSize                     : {}\n", value)?;
        }
        if let Some(value) = &self.TotalExcessFragments {
            write!(f, "TotalExcessFragments             : {}\n", value)?;
        }
        if let Some(value) = &self.TotalFiles {
            write!(f, "TotalFiles                       : {}\n", value)?;
        }
        if let Some(value) = &self.TotalFolders {
            write!(f, "TotalFolders                     : {}\n", value)?;
        }
        if let Some(value) = &self.TotalFragmentedFiles {
            write!(f, "TotalFragmentedFiles             : {}\n", value)?;
        }
        if let Some(value) = &self.TotalFreeSpaceExtents {
            write!(f, "TotalFreeSpaceExtents            : {}\n", value)?;
        }
        if let Some(value) = &self.TotalMFTFragments {
            write!(f, "TotalMFTFragments                : {}\n", value)?;
        }
        if let Some(value) = &self.TotalMFTSize {
            write!(f, "TotalMFTSize                     : {}\n", value)?;
        }
        if let Some(value) = &self.TotalPageFileFragments {
            write!(f, "TotalPageFileFragments           : {}\n", value)?;
        }
        if let Some(value) = &self.TotalPercentFragmentation {
            write!(f, "TotalPercentFragmentation        : {}\n", value)?;
        }
        if let Some(value) = &self.TotalUnmovableFiles {
            write!(f, "TotalUnmovableFiles              : {}\n", value)?;
        }
        if let Some(value) = &self.UsedSpace {
            write!(f, "UsedSpace                        : {}\n", value)?;
        }
        if let Some(value) = &self.VolumeName {
            write!(f, "VolumeName                       : {}\n", value)?;
        }
        if let Some(value) = &self.VolumeSize {
            write!(f, "VolumeSize                       : {}\n", value)?;
        }
        // Include system properties if necessary
        if let Some(value) = &self.__CLASS {
            write!(f, "__CLASS                          : {}\n", value)?;
        }
        if let Some(value) = &self.__DERIVATION {
            write!(f, "__DERIVATION                     : {:?}\n", value)?;
        }
        if let Some(value) = &self.__DYNASTY {
            write!(f, "__DYNASTY                        : {}\n", value)?;
        }
        if let Some(value) = &self.__GENUS {
            write!(f, "__GENUS                          : {}\n", value)?;
        }
        if let Some(value) = &self.__NAMESPACE {
            write!(f, "__NAMESPACE                      : {}\n", value)?;
        }
        if let Some(value) = &self.__PATH {
            write!(f, "__PATH                           : {}\n", value)?;
        }
        if let Some(value) = &self.__PROPERTY_COUNT {
            write!(f, "__PROPERTY_COUNT                 : {}\n", value)?;
        }
        if let Some(value) = &self.__RELPATH {
            write!(f, "__RELPATH                        : {}\n", value)?;
        }
        if let Some(value) = &self.__SERVER {
            write!(f, "__SERVER                         : {}\n", value)?;
        }
        if let Some(value) = &self.__SUPERCLASS {
            write!(f, "__SUPERCLASS                     : {}\n", value)?;
        }
        Ok(())
    }
}

/// Queries the `Win32_DefragAnalysis` WMI class to retrieve defragmentation
/// analysis information.
///
/// # Returns
///
/// A `Result` containing a vector of `Win32DefragAnalysis` instances if
/// successful, or a `UFFSError` if an error occurs.
///
/// # Errors
///
/// - `UFFSError::WMIQueryFailed`: If initializing the COM library, connecting
///   to WMI, or executing the WMI query fails.
/// - `UFFSError::EmptyDriveInfo`: If the query returns no results.
/// Query defragmentation analysis via WMI (Windows only)
#[cfg(windows)]
pub fn query_defrag_analysis() -> Result<Vec<Win32DefragAnalysis>, UFFSError> {
    // Establish a connection to WMI in the ROOT\CIMV2 namespace
    let wmi_con = WMIConnection::with_namespace_path("ROOT\\CIMV2").map_err(|e| {
        UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
    })?;

    // Define the WMI query to select all properties from Win32_DefragAnalysis
    let query = "SELECT * FROM Win32_DefragAnalysis";

    // Execute the query and collect the results
    let results: Vec<Win32DefragAnalysis> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if the results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the retrieved defragmentation analysis information
    Ok(results)
}
