//! Helper functions that wrap or re-implement FFmpeg's common utility routines. Keeping these
//! thin veneers in one place makes it easy to compare our Rust code with the original C
//! implementations from libavutil/libavformat when debugging.

use ffmpeg_sys_next::{av_dict_set, av_strerror, AVDictionary, AVRational, AV_ERROR_MAX_STRING_SIZE};
use std::collections::HashMap;
use std::ffi::{CStr, CString};

#[inline(always)]
fn ptr_u8_from_cstring(s: &CString) -> *const u8 {
    s.as_ptr() as *const u8
}

#[inline(always)]
fn ptr_u8_from_cstr(s: &CStr) -> *const u8 {
    s.as_ptr() as *const u8
}

#[inline(always)]
fn ptr_u8_from_bytes_z(b: &'static [u8]) -> *const u8 {
    b.as_ptr()
}

/// Convert an optional `HashMap<CString, CString>` into an `AVDictionary` by invoking
/// `av_dict_set()` for each entry.
///
/// FFmpeg reference: `av_dict_set()` in `libavutil/dict.c` uses the same ownership rules; the
/// caller is still responsible for freeing the resulting dictionary via `av_dict_free()`.
pub(crate) fn hashmap_to_avdictionary(opts: &Option<HashMap<CString, CString>>) -> *mut AVDictionary {
    let mut av_dict: *mut AVDictionary = std::ptr::null_mut();

    if let Some(map) = opts {
        for (key, value) in map {
            unsafe {
                av_dict_set(
                    &mut av_dict,
                    ptr_u8_from_cstring(key),
                    ptr_u8_from_cstring(value),
                    0,
                );
            }
        }
    }

    av_dict
}

/// Convert Rust String to C-compatible CString, handling null bytes.
#[allow(dead_code)]
pub(crate) fn string_to_cstring(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|e| format!("String contains null byte: {}", e))
}

/// Convert a `HashMap<String, String>` into an `AVDictionary`, validating that every key/value is
/// a valid C string before calling into FFmpeg.
///
/// FFmpeg reference: this mirrors the CLI path in `fftools/cmdutils.c` where user-facing strings
/// are validated and then passed to `av_dict_set()`.
#[allow(dead_code)]
pub(crate) fn hashmap_to_avdictionary_string(
    opts: &Option<HashMap<String, String>>,
) -> Result<*mut AVDictionary, String> {
    let mut av_dict: *mut AVDictionary = std::ptr::null_mut();

    if let Some(map) = opts {
        for (key, value) in map {
            let c_key = string_to_cstring(key)?;
            let c_value = string_to_cstring(value)?;
            unsafe {
                av_dict_set(
                    &mut av_dict,
                    ptr_u8_from_cstring(&c_key),
                    ptr_u8_from_cstring(&c_value),
                    0,
                );
            }
        }
    }

    Ok(av_dict)
}

/// Safe wrapper around `av_strerror()` that returns a Rust `String`.
///
/// FFmpeg reference: `av_strerror()` in `libavutil/error.c` uses a fixed-size buffer (defined by
/// `AV_ERROR_MAX_STRING_SIZE`), which we allocate on the stack and convert into UTF-8.
pub fn av_err2str(err: i32) -> String {
    unsafe {
        // Your bindgen output expects `*mut u8` for the buffer, not `*mut i8`.
        let mut buffer = [0u8; AV_ERROR_MAX_STRING_SIZE];
        av_strerror(err, buffer.as_mut_ptr(), AV_ERROR_MAX_STRING_SIZE);

        let c_str = CStr::from_ptr(buffer.as_ptr() as *const libc::c_char);
        match c_str.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => format!("Unknown error: {}", err),
        }
    }
}

/// Rust implementation of FFmpeg's `av_rescale_q_rnd`.
pub(crate) fn av_rescale_q_rnd(a: i64, bq: AVRational, cq: AVRational, rnd: u32) -> i64 {
    let b = bq.num as i64 * cq.den as i64;
    let c = cq.num as i64 * bq.den as i64;
    av_rescale_rnd(a, b, c, rnd)
}

/// Rust implementation of FFmpeg's `av_rescale_rnd`.
fn av_rescale_rnd(a: i64, b: i64, c: i64, mut rnd: u32) -> i64 {
    const AV_ROUND_PASS_MINMAX: u32 = ffmpeg_sys_next::AVRounding::AV_ROUND_PASS_MINMAX as u32;
    const INT_MAX: i64 = i32::MAX as i64;

    if c <= 0
        || b < 0
        || !((rnd & !AV_ROUND_PASS_MINMAX) <= 5 && (rnd & !AV_ROUND_PASS_MINMAX) != 4)
    {
        return i64::MIN;
    }

    if (rnd & AV_ROUND_PASS_MINMAX) != 0 {
        if a == i64::MIN || a == i64::MAX {
            return a;
        }
        rnd -= AV_ROUND_PASS_MINMAX;
    }

    if a < 0 {
        let neg_a = -a.max(-i64::MAX);
        let neg_result = av_rescale_rnd(neg_a, b, c, rnd ^ ((rnd >> 1) & 1));
        return -((neg_result as u64) as i64);
    }

    let r = if rnd == ffmpeg_sys_next::AVRounding::AV_ROUND_NEAR_INF as u32 {
        c / 2
    } else if (rnd & 1) != 0 {
        c - 1
    } else {
        0
    };

    if b <= INT_MAX && c <= INT_MAX {
        if a <= INT_MAX {
            return (a * b + r) / c;
        } else {
            let ad = a / c;
            let a2 = (a % c * b + r) / c;
            if ad >= INT_MAX && b != 0 && ad > (i64::MAX - a2) / b {
                return i64::MIN;
            }
            return ad * b + a2;
        }
    }

    rescale_large(a, b, c, r)
}

fn rescale_large(a: i64, b: i64, c: i64, r: i64) -> i64 {
    let a = a as u128;
    let b = b as u128;
    let c = c as u128;
    let r = r as u128;

    let result = (a * b + r) / c;

    if result > i64::MAX as u128 {
        i64::MIN
    } else {
        result as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_av_rescale_q_rnd_basic() {
        let bq = AVRational { num: 1, den: 1000 };
        let cq = AVRational { num: 1, den: 90000 };

        let result = av_rescale_q_rnd(1000, bq, cq, 5);
        assert_eq!(result, 90000);
    }

    #[test]
    fn test_av_rescale_q_rnd_with_pass_minmax_flag() {
        let bq = AVRational { num: 1, den: 1000 };
        let cq = AVRational { num: 1, den: 90000 };

        let result = av_rescale_q_rnd(i64::MIN, bq, cq, 8197);
        assert_eq!(result, i64::MIN);

        let result = av_rescale_q_rnd(i64::MAX, bq, cq, 8197);
        assert_eq!(result, i64::MAX);
    }

    #[test]
    fn test_av_rescale_q_rnd_normal_value_with_pass_minmax() {
        let bq = AVRational { num: 1, den: 1000 };
        let cq = AVRational { num: 1, den: 90000 };

        let result = av_rescale_q_rnd(1000, bq, cq, 8197);
        assert_eq!(result, 90000);
    }

    #[test]
    fn test_av_rescale_rnd_negative_value() {
        let result = av_rescale_rnd(-1000, 90000, 1000, 5);
        assert_eq!(result, -90000);
    }

    #[test]
    fn test_av_rescale_rnd_zero() {
        let result = av_rescale_rnd(0, 90000, 1000, 5);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_av_rescale_rnd_rounding_modes() {
        assert_eq!(av_rescale_rnd(7, 3, 5, 0), 4);
        assert_eq!(av_rescale_rnd(7, 3, 5, 1), 5);
        assert_eq!(av_rescale_rnd(7, 3, 5, 5), 4);
    }

    #[test]
    fn test_av_rescale_rnd_large_values() {
        let large_a = i32::MAX as i64 + 1000;
        let result = av_rescale_rnd(large_a, 1000, 1, 5);
        assert_eq!(result, large_a * 1000);
    }

    #[test]
    fn test_av_rescale_rnd_invalid_params() {
        assert_eq!(av_rescale_rnd(100, 100, 0, 5), i64::MIN);
        assert_eq!(av_rescale_rnd(100, -1, 100, 5), i64::MIN);
        assert_eq!(av_rescale_rnd(100, 100, 100, 4), i64::MIN);
    }
}