use crate::error::FindDevicesError;
use crate::error::FindDevicesError::MediaTypeSupported;
use crate::error::FindDevicesError::OsNotSupported;
use ffmpeg_sys_next::{av_input_audio_device_next, av_input_video_device_next, avdevice_free_list_devices, avdevice_list_input_sources, AVMediaType};
use std::ffi::CStr;
use std::ptr::{null, null_mut};

pub fn find_input_video_device_list() -> crate::error::Result<Vec<String>> {
    find_input_device_list(AVMediaType::AVMEDIA_TYPE_VIDEO)
}

pub fn find_input_audio_device_list() -> crate::error::Result<Vec<String>> {
    find_input_device_list(AVMediaType::AVMEDIA_TYPE_AUDIO)
}


fn find_input_device_list(media_type: AVMediaType) -> crate::error::Result<Vec<String>> {
    unsafe {
        crate::core::initialize_ffmpeg();

        let mut device_descriptions = Vec::new();

        let mut device_list = null_mut();

        let device_name = if media_type == AVMediaType::AVMEDIA_TYPE_VIDEO {
            if cfg!(target_os = "windows") {
                std::ffi::CString::new("dshow").unwrap()
            } else if cfg!(target_os = "linux") {
                std::ffi::CString::new("v4l2").unwrap()
            } else {
                return Err(OsNotSupported.into());
            }
        } else if media_type == AVMediaType::AVMEDIA_TYPE_AUDIO {
            if cfg!(target_os = "windows") {
                std::ffi::CString::new("dshow").unwrap()
            } else if cfg!(target_os = "linux") {
                std::ffi::CString::new("alsa").unwrap()
            } else {
                return Err(OsNotSupported.into());
            }
        } else {
            return Err(MediaTypeSupported(media_type as i32).into());
        };

        let ret = avdevice_list_input_sources(null(), device_name.as_ptr(), null_mut(), &mut device_list);
        if ret >= 0 && !device_list.is_null() {
            let nb_devices = (*device_list).nb_devices;

            for i in 0..nb_devices {
                let device = *(*device_list).devices.offset(i as isize);

                let nb_media_types = (*device).nb_media_types;
                let media_types = (*device).media_types;

                let mut flag = false;
                for j in 0..nb_media_types {
                    let device_media_type = *media_types.offset(j as isize);
                    if device_media_type == media_type {
                        flag = true;
                        break;
                    }
                }
                if !flag {
                    continue;
                }

                let result = CStr::from_ptr((*device).device_description).to_str();
                if let Err(e) = result {
                    return Err(FindDevicesError::UTF8Error.into());
                }
                let device_description = result.unwrap();
                device_descriptions.push(device_description.to_string());
            }
        }
        avdevice_free_list_devices(&mut device_list);

        Ok(device_descriptions)
    }
}