extern crate core_foundation;
extern crate objc;

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use objc::runtime::{Class, Object};
use objc::msg_send;
use objc::sel;
use objc::sel_impl;
use crate::error::FindDevicesError::AVCaptureDeviceNotFound;

pub fn get_avfoundation_devices(media_type: &str) -> crate::error::Result<Vec<String>> {
    let option =  Class::get("AVCaptureDevice");
    if let None = option {
        return Err(AVCaptureDeviceNotFound.into());
    }

    let av_capture_device_class = option.unwrap();

    // Convert media type to CFString
    let media_type_cf = CFString::new(media_type);

    // Call `AVCaptureDevice::devicesWithMediaType` to get the devices array
    let devices: *mut Object = unsafe {
        msg_send![av_capture_device_class, devicesWithMediaType: media_type_cf.as_concrete_TypeRef()]
    };

    let mut device_names = Vec::new();
    let devices_count: usize = unsafe {
        let count: usize = msg_send![devices, count];
        count
    };

    for i in 0..devices_count {
        // Get each device from the array
        let device: *mut Object = unsafe { msg_send![devices, objectAtIndex: i] };
        // Get device name
        let name: *const Object = unsafe { msg_send![device, localizedName] };
        let name_str = unsafe { CFString::wrap_under_get_rule(name as *const _).to_string() };
        device_names.push(name_str);
    }

    Ok(device_names)
}
