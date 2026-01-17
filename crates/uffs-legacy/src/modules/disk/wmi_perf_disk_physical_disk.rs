use std::fmt;

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use wmi::{COMLibrary, WMIConnection};

use crate::modules::errors::UFFSError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Win32PerfFormattedDataPerfDiskPhysicalDisk {
    /// The `AvgDiskBytesPerRead` property indicates the average bytes per read.
    AvgDiskBytesPerRead: Option<u64>,

    /// The `AvgDiskBytesPerTransfer` property indicates the average bytes per
    /// transfer.
    AvgDiskBytesPerTransfer: Option<u64>,

    /// The `AvgDiskBytesPerWrite` property indicates the average bytes per
    /// write.
    AvgDiskBytesPerWrite: Option<u64>,

    /// The `AvgDiskQueueLength` property indicates the average disk queue
    /// length.
    AvgDiskQueueLength: Option<u64>,

    /// The `AvgDiskReadQueueLength` property indicates the average read queue
    /// length.
    AvgDiskReadQueueLength: Option<u64>,

    /// The `AvgDisksecPerRead` property indicates the average seconds per read.
    AvgDisksecPerRead: Option<u32>,

    /// The `AvgDisksecPerTransfer` property indicates the average seconds per
    /// transfer.
    AvgDisksecPerTransfer: Option<u32>,

    /// The `AvgDisksecPerWrite` property indicates the average seconds per
    /// write.
    AvgDisksecPerWrite: Option<u32>,

    /// The `AvgDiskWriteQueueLength` property indicates the average write queue
    /// length.
    AvgDiskWriteQueueLength: Option<u64>,

    /// The `Caption` property provides a short textual description of the
    /// object.
    Caption: Option<String>,

    /// The `CurrentDiskQueueLength` property indicates the current disk queue
    /// length.
    CurrentDiskQueueLength: Option<u32>,

    /// The `Description` property contains a description of the object.
    Description: Option<String>,

    /// The `DiskBytesPersec` property indicates the number of bytes transferred
    /// per second.
    DiskBytesPersec: Option<u64>,

    /// The `DiskReadBytesPersec` property indicates the number of bytes read
    /// per second.
    DiskReadBytesPersec: Option<u64>,

    /// The `DiskReadsPersec` property indicates the number of read operations
    /// per second.
    DiskReadsPersec: Option<u32>,

    /// The `DiskTransfersPersec` property indicates the number of transfer
    /// operations per second.
    DiskTransfersPersec: Option<u32>,

    /// The `DiskWriteBytesPersec` property indicates the number of bytes
    /// written per second.
    DiskWriteBytesPersec: Option<u64>,

    /// The `DiskWritesPersec` property indicates the number of write operations
    /// per second.
    DiskWritesPersec: Option<u32>,

    /// The `Frequency_Object` property indicates the frequency of object
    /// updates.
    Frequency_Object: Option<u64>,

    /// The `Frequency_PerfTime` property indicates the frequency of performance
    /// counter updates.
    Frequency_PerfTime: Option<u64>,

    /// The `Frequency_Sys100NS` property indicates the frequency of system
    /// updates.
    Frequency_Sys100NS: Option<u64>,

    /// The `Name` property contains the name of the physical disk instance.
    Name: Option<String>,

    /// The `PercentDiskReadTime` property indicates the percentage of time
    /// spent on disk read operations.
    PercentDiskReadTime: Option<u64>,

    /// The `PercentDiskTime` property indicates the percentage of time spent on
    /// disk operations.
    PercentDiskTime: Option<u64>,

    /// The `PercentDiskWriteTime` property indicates the percentage of time
    /// spent on disk write operations.
    PercentDiskWriteTime: Option<u64>,

    /// The `PercentIdleTime` property indicates the percentage of time the disk
    /// was idle.
    PercentIdleTime: Option<u64>,

    /// The `SplitIOPerSec` property indicates the number of split I/O
    /// operations per second.
    SplitIOPerSec: Option<u32>,

    /// The `Timestamp_Object` property indicates the timestamp of the object.
    Timestamp_Object: Option<u64>,

    /// The `Timestamp_PerfTime` property indicates the timestamp of the
    /// performance data.
    Timestamp_PerfTime: Option<u64>,

    /// The `Timestamp_Sys100NS` property indicates the timestamp of the system
    /// updates.
    Timestamp_Sys100NS: Option<u64>,

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
impl fmt::Display for Win32PerfFormattedDataPerfDiskPhysicalDisk {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(name) = &self.Name {
            write!(f, "Name                     : {}\n", name)?;
        }
        if let Some(avg_disk_bytes_per_read) = &self.AvgDiskBytesPerRead {
            write!(
                f,
                "AvgDiskBytesPerRead      : {}\n",
                avg_disk_bytes_per_read
            )?;
        }
        if let Some(avg_disk_bytes_per_transfer) = &self.AvgDiskBytesPerTransfer {
            write!(
                f,
                "AvgDiskBytesPerTransfer  : {}\n",
                avg_disk_bytes_per_transfer
            )?;
        }
        if let Some(avg_disk_bytes_per_write) = &self.AvgDiskBytesPerWrite {
            write!(
                f,
                "AvgDiskBytesPerWrite     : {}\n",
                avg_disk_bytes_per_write
            )?;
        }
        if let Some(avg_disk_queue_length) = &self.AvgDiskQueueLength {
            write!(f, "AvgDiskQueueLength       : {}\n", avg_disk_queue_length)?;
        }
        if let Some(avg_disk_read_queue_length) = &self.AvgDiskReadQueueLength {
            write!(
                f,
                "AvgDiskReadQueueLength   : {}\n",
                avg_disk_read_queue_length
            )?;
        }
        if let Some(avg_disksec_per_read) = &self.AvgDisksecPerRead {
            write!(f, "AvgDisksecPerRead        : {}\n", avg_disksec_per_read)?;
        }
        if let Some(avg_disksec_per_transfer) = &self.AvgDisksecPerTransfer {
            write!(
                f,
                "AvgDisksecPerTransfer    : {}\n",
                avg_disksec_per_transfer
            )?;
        }
        if let Some(avg_disksec_per_write) = &self.AvgDisksecPerWrite {
            write!(f, "AvgDisksecPerWrite       : {}\n", avg_disksec_per_write)?;
        }
        if let Some(current_disk_queue_length) = &self.CurrentDiskQueueLength {
            write!(
                f,
                "CurrentDiskQueueLength   : {}\n",
                current_disk_queue_length
            )?;
        }
        if let Some(disk_bytes_persec) = &self.DiskBytesPersec {
            write!(f, "DiskBytesPersec          : {}\n", disk_bytes_persec)?;
        }
        if let Some(disk_read_bytes_persec) = &self.DiskReadBytesPersec {
            write!(f, "DiskReadBytesPersec      : {}\n", disk_read_bytes_persec)?;
        }
        if let Some(disk_reads_persec) = &self.DiskReadsPersec {
            write!(f, "DiskReadsPersec          : {}\n", disk_reads_persec)?;
        }
        if let Some(disk_transfers_persec) = &self.DiskTransfersPersec {
            write!(f, "DiskTransfersPersec      : {}\n", disk_transfers_persec)?;
        }
        if let Some(disk_write_bytes_persec) = &self.DiskWriteBytesPersec {
            write!(
                f,
                "DiskWriteBytesPersec     : {}\n",
                disk_write_bytes_persec
            )?;
        }
        if let Some(disk_writes_persec) = &self.DiskWritesPersec {
            write!(f, "DiskWritesPersec         : {}\n", disk_writes_persec)?;
        }
        if let Some(percent_disk_read_time) = &self.PercentDiskReadTime {
            write!(f, "PercentDiskReadTime      : {}\n", percent_disk_read_time)?;
        }
        if let Some(percent_disk_time) = &self.PercentDiskTime {
            write!(f, "PercentDiskTime          : {}\n", percent_disk_time)?;
        }
        if let Some(percent_disk_write_time) = &self.PercentDiskWriteTime {
            write!(
                f,
                "PercentDiskWriteTime     : {}\n",
                percent_disk_write_time
            )?;
        }
        if let Some(percent_idle_time) = &self.PercentIdleTime {
            write!(f, "PercentIdleTime          : {}\n", percent_idle_time)?;
        }
        if let Some(split_io_per_sec) = &self.SplitIOPerSec {
            write!(f, "SplitIOPerSec            : {}\n", split_io_per_sec)?;
        }
        if let Some(class) = &self.__CLASS {
            write!(f, "__CLASS                  : {}\n", class)?;
        }
        if let Some(derivation) = &self.__DERIVATION {
            write!(f, "__DERIVATION             : {:?}\n", derivation)?;
        }
        if let Some(dynasty) = &self.__DYNASTY {
            write!(f, "__DYNASTY                : {}\n", dynasty)?;
        }
        if let Some(genus) = &self.__GENUS {
            write!(f, "__GENUS                  : {}\n", genus)?;
        }
        if let Some(namespace) = &self.__NAMESPACE {
            write!(f, "__NAMESPACE              : {}\n", namespace)?;
        }
        if let Some(path) = &self.__PATH {
            write!(f, "__PATH                   : {}\n", path)?;
        }
        if let Some(property_count) = &self.__PROPERTY_COUNT {
            write!(f, "__PROPERTY_COUNT         : {}\n", property_count)?;
        }
        if let Some(server) = &self.__SERVER {
            write!(f, "__SERVER                 : {}\n", server)?;
        }
        if let Some(superclass) = &self.__SUPERCLASS {
            write!(f, "__SUPERCLASS             : {}\n", superclass)?;
        }
        Ok(())
    }
}

/// Query physical disk performance data via WMI (Windows only)
#[cfg(windows)]
pub fn query_perf_disk_physical_disk()
-> Result<Vec<Win32PerfFormattedDataPerfDiskPhysicalDisk>, UFFSError> {
    // Initialize COM library
    let com_con = COMLibrary::new()
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to initialize COM: {:?}", e)))?;

    // Establish a connection to WMI in the correct namespace
    let wmi_con =
        WMIConnection::with_namespace_path("ROOT\\CIMv2", com_con.into()).map_err(|e| {
            UFFSError::WMIQueryFailed(format!("Failed to connect to WMI namespace: {:?}", e))
        })?;

    // Define the WMI query
    let query = "SELECT * FROM Win32_PerfFormattedData_PerfDisk_PhysicalDisk";

    // Execute the query and get results
    let results: Vec<Win32PerfFormattedDataPerfDiskPhysicalDisk> = wmi_con
        .raw_query(query)
        .map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to execute WMI query: {:?}", e)))?;

    // Check if results are empty
    if results.is_empty() {
        return Err(UFFSError::EmptyDriveInfo);
    }

    // Return the results
    Ok(results)
}
