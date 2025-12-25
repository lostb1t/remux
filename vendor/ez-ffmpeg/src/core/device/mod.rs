#[cfg(target_os = "macos")]
mod macos_avfoundation;

#[cfg(not(target_os = "macos"))]
mod avdevice;

/// Retrieves a list of available video input devices (e.g., cameras) on the system.
///
/// This function attempts to query the available video devices that can be used as input,
/// similar to the command `ffmpeg -list_devices`. The function behavior is dependent on the
/// operating system being used:
///
/// - **macOS**: Uses AVFoundation to gather information on available video devices.
///   The AVFoundation framework allows direct access to multimedia devices,
///   and the function specifically filters for "vide" devices, which represent video input sources.
/// - **Other Operating Systems**: Calls a general `avdevice` function to obtain a list of
///   video input devices using FFmpeg's device capabilities.
///
/// # Returns
///
/// A `Result` containing a `Vec<String>` with the names of available video input devices.
/// If successful, the vector contains the names of devices such as "FaceTime HD Camera".
/// If an error occurs, it returns a custom error type defined in `crate::error`.
///
/// # Example
///
/// ```rust
/// let video_devices = get_input_video_devices()?;
/// for device in video_devices {
///     println!("Available video device: {}", device);
/// }
/// ```
///
/// # Errors
///
/// This function returns an error if the device query process fails on any platform.
pub fn get_input_video_devices() -> crate::error::Result<Vec<String>> {
    #[cfg(target_os = "macos")]
    return macos_avfoundation::get_avfoundation_devices("vide");

    #[cfg(not(target_os = "macos"))]
    avdevice::find_input_video_device_list()
}

/// Retrieves a list of available audio input devices (e.g., microphones) on the system.
///
/// This function attempts to query the available audio devices that can be used as input,
/// similar to the command `ffmpeg -list_devices`. The function behavior depends on the
/// operating system being used:
///
/// - **macOS**: Uses AVFoundation to gather information on available audio devices.
///   The AVFoundation framework allows direct access to multimedia devices,
///   and the function specifically filters for "soun" devices, which represent audio input sources.
/// - **Other Operating Systems**: Calls a general `avdevice` function to obtain a list of
///   audio input devices using FFmpeg's device capabilities.
///
/// # Returns
///
/// A `Result` containing a `Vec<String>` with the names of available audio input devices.
/// If successful, the vector contains the names of devices such as "Built-in Microphone".
/// If an error occurs, it returns a custom error type defined in `crate::error`.
///
/// # Example
///
/// ```rust
/// let audio_devices = get_input_audio_devices()?;
/// for device in audio_devices {
///     println!("Available audio device: {}", device);
/// }
/// ```
///
/// # Errors
///
/// This function returns an error if the device query process fails on any platform.
pub fn get_input_audio_devices() -> crate::error::Result<Vec<String>> {
    #[cfg(target_os = "macos")]
    return macos_avfoundation::get_avfoundation_devices("soun");

    #[cfg(not(target_os = "macos"))]
    avdevice::find_input_audio_device_list()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_input_video_devices() {
        let video_devices = get_input_video_devices().unwrap();
        println!("{:?}", video_devices);
    }

    #[test]
    fn test_get_input_audio_devices() {
        let audio_devices = get_input_audio_devices().unwrap();
        println!("{:?}", audio_devices);
    }
}