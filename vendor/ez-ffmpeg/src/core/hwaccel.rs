use ffmpeg_sys_next::{
    av_buffer_unref, av_dict_parse_string, av_hwdevice_ctx_create, av_hwdevice_ctx_create_derived,
    av_hwdevice_find_type_by_name, av_hwdevice_get_type_name, av_hwdevice_iterate_types,
    avcodec_get_hw_config, AVBufferRef, AVCodec, AVHWDeviceType, AVERROR,
    AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX, EINVAL, ENOMEM,
};
use log::{error, warn};
use std::ffi::{CStr, CString};
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct HWAccelInfo {
    pub name: String,
    pub hw_device_type: AVHWDeviceType,
}

pub fn get_hwaccels() -> Vec<HWAccelInfo> {
    let mut hwaccels = Vec::new();
    let mut device_type = AVHWDeviceType::AV_HWDEVICE_TYPE_NONE;

    loop {
        device_type = unsafe { av_hwdevice_iterate_types(device_type) };
        if device_type == AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            break;
        }

        let name = unsafe {
            let name = av_hwdevice_get_type_name(device_type);
            match CStr::from_ptr(name).to_str() {
                Ok(name) => name.to_string(),
                Err(_) => "unknown name".to_string(),
            }
        };

        hwaccels.push(HWAccelInfo {
            name,
            hw_device_type: device_type,
        });
    }

    hwaccels
}

static HW_DEVICES: OnceLock<Mutex<Vec<HWDevice>>> = OnceLock::new();
static FILTER_HW_DEVICE: OnceLock<Mutex<Option<HWDevice>>> = OnceLock::new();

pub(crate) fn new_hw_devices() -> Mutex<Vec<HWDevice>> {
    Mutex::new(Vec::new())
}

pub(crate) fn init_filter_hw_device(hw_device: &str) -> i32 {
    if FILTER_HW_DEVICE.get().is_some() {
        warn!("Only one filter device can be used.");
        return 0;
    }
    match hw_device_init_from_string(hw_device) {
        (err, Some(dev)) if err == 0 => {
            FILTER_HW_DEVICE.set(Mutex::new(Some(dev.clone()))).ok();
            0
        }
        (_, _) => {
            error!("Invalid filter device {}", hw_device);
            FILTER_HW_DEVICE.set(Mutex::new(None)).ok();
            AVERROR(EINVAL)
        }
    }
}

#[repr(i32)]
#[derive(Copy, Clone, PartialEq)]
pub enum HWAccelID {
    HwaccelNone = 0,
    HwaccelAuto,
    HwaccelGeneric,
}

#[derive(Clone, Debug)]
pub(crate) struct HWDevice {
    pub(crate) name: String,
    pub(crate) device_type: AVHWDeviceType,
    pub(crate) device_ref: *mut AVBufferRef,
}

unsafe impl Send for HWDevice {}
unsafe impl Sync for HWDevice {}

pub(crate) unsafe fn hw_device_free_all() {
    // Free the global filter hardware device
    if let Some(filter_device) = FILTER_HW_DEVICE.get() {
        match filter_device.lock() {
            Ok(mut device_guard) => {
                if let Some(device) = device_guard.as_mut() {
                    // Check if device reference is valid to avoid double free
                    if !device.device_ref.is_null() {
                        av_buffer_unref(&mut device.device_ref);
                        // Note: av_buffer_unref will set the pointer to null
                    }
                }
            }
            Err(e) => {
                error!("Failed to lock global filter hardware device: {}", e);
            }
        }
    }

    // Free all devices in the hardware device list
    if let Some(hw_devices) = HW_DEVICES.get() {
        match hw_devices.lock() {
            Ok(mut devices_guard) => {
                // Iterate through and free each device reference
                for device in devices_guard.iter_mut() {
                    if !device.device_ref.is_null() {
                        av_buffer_unref(&mut device.device_ref);
                        // av_buffer_unref automatically sets pointer to null to prevent dangling pointers
                    }
                }
                // Optional: Clear the device list to free Vec memory
                devices_guard.clear();
            }
            Err(e) => {
                error!("Failed to lock hardware device list: {}", e);
            }
        }
    }
}

pub(crate) fn hw_device_for_filter() -> Option<HWDevice> {
    if let Some(dev) = FILTER_HW_DEVICE.get() {
        let dev_option = dev.lock().unwrap();
        if let Some(dev) = dev_option.as_ref() {
            return Some(dev.clone());
        }
    }
    let devices = HW_DEVICES.get_or_init(|| new_hw_devices());

    let devices = devices.lock().unwrap();
    if !devices.is_empty() {
        let dev = devices.last();

        match dev {
            None => {}
            Some(dev) => {
                if devices.len() > 1 {
                    unsafe {
                        let type_name = av_hwdevice_get_type_name(dev.device_type);
                        let type_name = CStr::from_ptr(type_name).to_str();
                        if let Ok(type_name) = type_name {
                            warn!("There are {} hardware devices. device {} of type {type_name} is picked for filters by default. Set hardware device explicitly with the filter_hw_device option if device {} is not usable for filters.",
                            devices.len(),dev.name,
                            dev.name,);
                        }
                    }
                }

                return Some(dev.clone());
            }
        }
    }

    None
}

pub(crate) fn hw_device_match_by_codec(codec: *const AVCodec) -> Option<HWDevice> {
    let mut i = 0;

    loop {
        let config = unsafe { avcodec_get_hw_config(codec, i) };
        if config.is_null() {
            return None;
        }

        unsafe {
            if (*config).methods as u32 & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as u32 == 0 {
                i += 1;
                continue;
            }

            if let Some(dev) = hw_device_get_by_type((*config).device_type) {
                return Some(dev.clone());
            }
        }

        i += 1;
    }
}

pub(crate) fn hw_device_get_by_type(device_type: AVHWDeviceType) -> Option<HWDevice> {
    let mut found = None;

    let devices = HW_DEVICES.get_or_init(|| new_hw_devices());
    let devices = devices.lock().unwrap();
    for device in devices.iter() {
        if device.device_type == device_type {
            if found.is_some() {
                return None;
            }
            found = Some(device.clone());
        }
    }
    found
}

pub(crate) fn hw_device_init_from_string(arg: &str) -> (i32, Option<HWDevice>) {
    let mut device_ref = null_mut();

    let k = arg
        .find(|c| c == ':' || c == '=' || c == '@')
        .unwrap_or(arg.len());
    let mut p = &arg[k..];

    let Ok(type_name) = CString::new(p) else {
        error!("Device creation failed: type:{p} can't convert to CString");
        return (AVERROR(ENOMEM), None);
    };
    let device_type = unsafe { av_hwdevice_find_type_by_name(type_name.as_ptr()) };
    if device_type == AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
        error!("Invalid device specification \"{arg}\": unknown device type");
        return (AVERROR(EINVAL), None);
    }

    let name = if p.starts_with('=') {
        let name_end = p[1..]
            .find(|c| c == ':' || c == '@' || c == ',')
            .unwrap_or(p.len() - 1);
        let name = Some(p[1..=name_end].to_string());

        if hw_device_get_by_name(&name.clone().unwrap()).is_some() {
            error!("Invalid device specification \"{arg}\": named device already exists");
            return (AVERROR(EINVAL), None);
        }

        let new_p_index = 1 + name_end;
        p = &p[new_p_index..];
        name
    } else {
        hw_device_default_name(device_type)
    };

    if p.is_empty() {
        // New device with no parameters.
        let err =
            unsafe { av_hwdevice_ctx_create(&mut device_ref, device_type, null(), null_mut(), 0) };
        if err < 0 {
            error!("Device creation failed: {err}.");
            unsafe {
                av_buffer_unref(&mut device_ref);
            }
            return (err, None);
        }
    } else if p.starts_with(':') {
        // New device with some parameters.
        let mut device_name: Option<String> = None;
        let mut options = null_mut();

        if let Some(comma_pos) = p.find(',') {
            if comma_pos > 0 {
                device_name = Some(p[..comma_pos].to_string());
            }
            unsafe {
                let v = &p[comma_pos + 1..];
                let Ok(v_cstr) = CString::new(v) else {
                    error!("Device creation failed: option:{v} can't convert to CString");
                    av_buffer_unref(&mut device_ref);
                    return (AVERROR(EINVAL), None);
                };
                let eq_cstr = CString::new("=").unwrap();
                let comma_cstr = CString::new(",").unwrap();
                let err = av_dict_parse_string(
                    &mut options,
                    v_cstr.as_ptr(),
                    eq_cstr.as_ptr(),
                    comma_cstr.as_ptr(),
                    0,
                );
                if err < 0 {
                    error!("Invalid device specification \"{arg}\": failed to parse options");
                    av_buffer_unref(&mut device_ref);
                    return (AVERROR(EINVAL), None);
                }
            }
        } else {
            if !p.is_empty() {
                device_name = Some(p.to_string());
            }
        }

        let err = unsafe {
            match device_name {
                None => av_hwdevice_ctx_create(&mut device_ref, device_type, null(), options, 0),
                Some(device_name) => {
                    let Ok(device_name_cstr) = CString::new(device_name.clone()) else {
                        error!("Device creation failed: device_name:{device_name} can't convert to CString");
                        av_buffer_unref(&mut device_ref);
                        return (AVERROR(EINVAL), None);
                    };
                    av_hwdevice_ctx_create(
                        &mut device_ref,
                        device_type,
                        device_name_cstr.as_ptr(),
                        options,
                        0,
                    )
                }
            }
        };
        if err < 0 {
            error!("Device creation failed: {err}.");
            unsafe {
                av_buffer_unref(&mut device_ref);
            }
            return (err, None);
        }
    } else if p.starts_with('@') {
        // Derive from existing device.
        let src_name = &p[1..];
        let Some(src_device) = hw_device_get_by_name(src_name) else {
            error!("Invalid device specification \"{arg}\": invalid source device name");
            unsafe {
                av_buffer_unref(&mut device_ref);
            }
            return (AVERROR(EINVAL), None);
        };
        let err = unsafe {
            av_hwdevice_ctx_create_derived(&mut device_ref, device_type, src_device.device_ref, 0)
        };
        if err < 0 {
            error!("Device creation failed: {err}.");
            unsafe {
                av_buffer_unref(&mut device_ref);
            }
            return (err, None);
        }
    } else if p.starts_with(',') {
        unsafe {
            let mut options = null_mut();
            let v = &p[1..];
            let Ok(v_cstr) = CString::new(v) else {
                error!("Device creation failed: option:{v} can't convert to CString");
                av_buffer_unref(&mut device_ref);
                return (AVERROR(EINVAL), None);
            };
            let eq_cstr = CString::new("=").unwrap();
            let comma_cstr = CString::new(",").unwrap();
            let mut err = av_dict_parse_string(
                &mut options,
                v_cstr.as_ptr(),
                eq_cstr.as_ptr(),
                comma_cstr.as_ptr(),
                0,
            );
            if err < 0 {
                error!("Invalid device specification \"{arg}\": failed to parse options");
                av_buffer_unref(&mut device_ref);
                return (AVERROR(EINVAL), None);
            }
            err = av_hwdevice_ctx_create(&mut device_ref, device_type, null(), options, 0);
            if err < 0 {
                error!("Device creation failed: {err}.");
                av_buffer_unref(&mut device_ref);
                return (err, None);
            }
        }
    } else {
        error!("Invalid device specification \"{arg}\": parse error");
        return (AVERROR(EINVAL), None);
    }

    let dev = HWDevice {
        name: name.unwrap(),
        device_type,
        device_ref,
    };
    add_hw_device(dev.clone());

    (0, Some(dev))
}

pub(crate) fn hw_device_init_from_type(
    device_type: AVHWDeviceType,
    device: Option<String>,
) -> (i32, Option<HWDevice>) {
    let name = hw_device_default_name(device_type);
    if name.is_none() {
        return (AVERROR(ENOMEM), None);
    }

    let mut device_ref = null_mut();

    let err = match device {
        None => unsafe {
            av_hwdevice_ctx_create(&mut device_ref, device_type, null(), null_mut(), 0)
        },
        Some(device) => {
            let Ok(device_cstr) = CString::new(device) else {
                return (AVERROR(EINVAL), None);
            };

            unsafe {
                av_hwdevice_ctx_create(
                    &mut device_ref,
                    device_type,
                    device_cstr.as_ptr(),
                    null_mut(),
                    0,
                )
            }
        }
    };

    if err < 0 {
        error!("Device creation failed: {}.", err);
        unsafe {
            av_buffer_unref(&mut device_ref);
        }
        return (err, None);
    }

    let dev = HWDevice {
        name: name.unwrap(),
        device_type,
        device_ref,
    };

    add_hw_device(dev.clone());

    (0, Some(dev))
}

pub(crate) fn hw_device_default_name(device_type: AVHWDeviceType) -> Option<String> {
    // Get the name of the hardware device type
    let type_name = unsafe { av_hwdevice_get_type_name(device_type) };
    if type_name.is_null() {
        return None;
    }

    let type_name = unsafe { CStr::from_ptr(type_name) }.to_str().ok()?;
    let index_limit = 1000;

    for index in 0..index_limit {
        let name = format!("{}{}", type_name, index);

        // Check if the name is available
        if hw_device_get_by_name(&name).is_none() {
            return Some(name);
        }
    }

    None
}

pub(crate) fn hw_device_get_by_name(name: &str) -> Option<HWDevice> {
    let devices = HW_DEVICES.get_or_init(|| new_hw_devices());

    let devices = devices.lock().unwrap();
    for device in devices.iter() {
        if device.name == name {
            return Some(device.clone());
        }
    }

    None
}

fn add_hw_device(device: HWDevice) {
    let devices = HW_DEVICES.get_or_init(|| new_hw_devices());
    let mut devices = devices.lock().unwrap();
    devices.push(device);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_hwaccels() {
        let hwaccels = get_hwaccels();
        println!("{:?}", hwaccels);
    }
}
