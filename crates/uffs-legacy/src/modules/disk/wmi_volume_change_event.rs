// use serde::{Deserialize, Serialize};
// use std::{fmt, ptr};
// use std::result::Result as StdResult; // Import standard Result with alias
//
//
// use windows::{
//     core::{GUID, Interface, HRESULT, BSTR, Result, implement, VARIANT},
//     Win32::System::Com::{
//         CoInitializeEx, CoSetProxyBlanket, CoCreateInstance,
// CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,         RPC_C_AUTHN_LEVEL_CALL,
// RPC_C_IMP_LEVEL_IMPERSONATE, EOLE_AUTHENTICATION_CAPABILITIES,     },
//     Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE},
//     Win32::System::Wmi::{IWbemClassObject, IWbemLocator, IWbemObjectSink,
// IWbemObjectSink_Impl, WBEM_GENERIC_FLAG_TYPE},
//     Win32::System::Variant::VariantClear
// };
//
// use crate::modules::errors::UFFSError;
//
// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// pub struct Win32VolumeChangeEvent {
//     pub SECURITY_DESCRIPTOR: Option<Vec<u8>>,
//     pub TIME_CREATED: Option<u64>,
//     pub EventType: Option<u16>,
//     pub DriveName: Option<String>,
// }
//
// impl fmt::Display for Win32VolumeChangeEvent {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         if let Some(event_type) = &self.EventType {
//             write!(f, "EventType        : {}\n", event_type)?;
//         }
//         if let Some(drive_name) = &self.DriveName {
//             write!(f, "DriveName        : {}\n", drive_name)?;
//         }
//         if let Some(time_created) = &self.TIME_CREATED {
//             write!(f, "TIME_CREATED     : {}\n", time_created)?;
//         }
//         Ok(())
//     }
// }
//
// #[implement(IWbemObjectSink)]
// struct VolumeChangeEventSink {
//     callback: Box<dyn Fn(Win32VolumeChangeEvent) + Send + Sync>,
// }
//
// impl VolumeChangeEventSink {
//     fn new(callback: Box<dyn Fn(Win32VolumeChangeEvent) + Send + Sync>) ->
// Self {         Self { callback }
//     }
// }
//
// #[allow(non_snake_case)]
// impl IWbemObjectSink_Impl for VolumeChangeEventSink {
//     fn Indicate(
//         &self,
//         lObjectCount: i32,
//         apObjArray: *const Option<IWbemClassObject>,
//     ) -> Result<()> {
//         unsafe {
//             for i in 0..lObjectCount {
//                 let pObj = (*apObjArray.offset(i as isize)).as_ref();
//                 if let Some(pObj) = pObj {
//                     if let Ok(event) = parse_volume_change_event(pObj) {
//                         (self.callback)(event);
//                     }
//                 }
//             }
//         }
//         Ok(())
//     }
//
//     fn SetStatus(
//         &self,
//         _lFlags: i32,
//         _hResult: HRESULT,
//         _strParam: &BSTR,
//         _pObjParam: Option<&IWbemClassObject>,
//     ) -> Result<()> {
//         Ok(())
//     }
// }
//
// fn parse_volume_change_event(pObj: &IWbemClassObject) ->
// StdResult<Win32VolumeChangeEvent, UFFSError> {     let mut event_type = None;
//     let mut drive_name = None;
//     let mut time_created = None;
//
//     // EventType
//     unsafe {
//         let mut var_event_type: VARIANT = VARIANT::default();
//         pObj.Get(
//             "EventType",
//             0,
//             &mut var_event_type,
//             *ptr::null_mut(),
//             *ptr::null_mut(),
//         )
//             .map_err(|_| UFFSError::GetPropertyError("Failed to get EventType
// property".into()))?;
//
//         let value = u16::try_from(&var_event_type)
//             .map_err(|_| {
//                 if let Err(err) = VariantClear(&mut var_event_type) {
//                     return Err(UFFSError::WindowsApiError(format!("Failed to
// clear VARIANT: {:?}", err)));                 };
//                 Err(UFFSError::VariantConversionError("Failed to convert
// VARIANT to u16".into()))             })?;
//
//         if let Err(err) = VariantClear(&mut var_event_type) {
//             return Err(UFFSError::WindowsApiError(format!("Failed to clear
// VARIANT: {:?}", err)));         }
//         event_type = Some(value);
//         println!("Event Type: {}", value);
//     }
//
//     // DriveName
//     unsafe {
//         let mut var_drive_name: VARIANT = VARIANT::default();
//         pObj.Get(
//             "DriveName",
//             0,
//             &mut var_drive_name,
//             *ptr::null_mut(),
//             *ptr::null_mut(),
//         )
//             .map_err(|_| UFFSError::GetPropertyError("Failed to get DriveName
// property".into()))?;
//
//         let bstr_value = BSTR::try_from(&var_drive_name)
//             .map_err(|_| {
//                 if let Err(err) = VariantClear(&mut var_drive_name) {
//                     return Err(UFFSError::WindowsApiError(format!("Failed to
// clear VARIANT: {:?}", err)));                 }
//                 Err(UFFSError::VariantConversionError("Failed to convert
// VARIANT to BSTR".into()))             })?;
//
//         if let Err(err) = VariantClear(&mut var_drive_name) {
//             return Err(UFFSError::WindowsApiError(format!("Failed to clear
// VARIANT: {:?}", err)));         }
//         drive_name = Some(bstr_value.to_string());
//         println!("Drive Name: {}", drive_name.as_ref().unwrap());
//     }
//
//     // TIME_CREATED
//     unsafe {
//         let mut var_time_created: VARIANT = VARIANT::default();
//         pObj.Get(
//             "TIME_CREATED",
//             0,
//             &mut var_time_created,
//             *ptr::null_mut(),
//             *ptr::null_mut(),
//         )
//             .map_err(|_| UFFSError::GetPropertyError("Failed to get
// TIME_CREATED property".into()))?;
//
//         let value = u64::try_from(&var_time_created)
//             .map_err(|_| {
//                 if let Err(err) = VariantClear(&mut var_time_created) {
//                     return Err(UFFSError::WindowsApiError(format!("Failed to
// clear VARIANT: {:?}", err)));                 }
//                 Err(UFFSError::VariantConversionError("Failed to convert
// VARIANT to u64".into()))             })?;
//
//         if let Err(err) = VariantClear(&mut var_time_created) {
//             return Err(UFFSError::WindowsApiError(format!("Failed to clear
// VARIANT: {:?}", err)));         }
//         time_created = Some(value);
//         println!("Time Created: {}", value);
//     }
//
//     Ok(Win32VolumeChangeEvent {
//         SECURITY_DESCRIPTOR: None,
//         TIME_CREATED: time_created,
//         EventType: event_type,
//         DriveName: drive_name,
//     })
// }
//
// pub fn subscribe_to_volume_change_events<F>(callback: F) -> StdResult<(),
// UFFSError> where
//     F: Fn(Win32VolumeChangeEvent) + Send + Sync + 'static,
// {
//     unsafe {
//         // Initialize COM library
//         const CLSID_WbemLocator: GUID =
// GUID::from_u128(0x4590f811_1d3a_11d0_891f_00aa004b2e24);
//
//         CoInitializeEx(None, COINIT_MULTITHREADED)
//             .ok()  // Convert HRESULT to Result
//             .map_err(|e| UFFSError::WindowsApiError(format!("Failed to
// initialize COM library: {:?}", e)))?;
//
//         // Obtain the initial locator to WMI
//         let locator: IWbemLocator = CoCreateInstance(&CLSID_WbemLocator,
// None, CLSCTX_INPROC_SERVER)             .map_err(|e|
// UFFSError::WindowsApiError(format!("Failed to create IWbemLocator instance:
// {:?}", e)))?;
//
//         // Connect to WMI namespace
//         let services = locator.ConnectServer(
//             "ROOT\\CIMV2",
//             None,
//             None,
//             None,
//             0x10,  // Use 16 (0x10) for WBEM_FLAG_RETURN_IMMEDIATELY
//             None,
//             None,
//         ).map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to connect to
// WMI namespace: {:?}", e)))?;
//
//         // Set security levels on the proxy
//         let hr = CoSetProxyBlanket(
//             &services,
//             RPC_C_AUTHN_WINNT,
//             RPC_C_AUTHZ_NONE,
//             None,
//             RPC_C_AUTHN_LEVEL_CALL,
//             RPC_C_IMP_LEVEL_IMPERSONATE,
//             None,
//             EOLE_AUTHENTICATION_CAPABILITIES(0), // Use
// EOLE_AUTHENTICATION_CAPABILITIES initialized with 0         );
//
//         if hr.is_err() {
//             return Err(UFFSError::WindowsApiError(format!("Failed to set
// proxy blanket: {:?}", hr)));         }
//
//         // Create the event sink
//         let sink = VolumeChangeEventSink::new(Box::new(callback));
//         let sink_ptr = sink.into();
//
//         // Set up the event subscription
//         services.ExecNotificationQueryAsync(
//             "WQL",
//             "SELECT * FROM Win32_VolumeChangeEvent",
//             WBEM_GENERIC_FLAG_TYPE(0x10),  // Use WBEM_GENERIC_FLAG_TYPE
// wrapper around 0x10             None,
//             &sink_ptr.cast().map_err(|e|
// UFFSError::WindowsApiError(format!("Failed to cast sink pointer: {:?}",
// e)))?,         ).map_err(|e| UFFSError::WMIQueryFailed(format!("Failed to set
// up event subscription: {:?}", e)))?;
//
//         // Keep the application running to receive events
//         println!("Subscribed to volume change events. Press Ctrl+C to
// exit.");         loop {
//             std::thread::park();
//         }
//     }
// }
