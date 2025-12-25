use ez_ffmpeg::{FfmpegContext, Input, Output};
use std::sync::{Arc, Mutex};

fn main() {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};

    // Specify input and output file paths
    let input_file = "test.mp4";
    let output_file = "output.mp4";

    // Open the input file in a thread-safe manner using Arc and Mutex
    let input_file = Arc::new(Mutex::new(
        File::open(input_file).expect("Failed to open input file"),
    ));

    // Define the read callback for custom input handling
    let read_callback: Box<dyn FnMut(&mut [u8]) -> i32> = {
        let input = Arc::clone(&input_file);
        Box::new(move |buf: &mut [u8]| -> i32 {
            let mut input = input.lock().unwrap();
            match input.read(buf) {
                Ok(0) => ffmpeg_sys_next::AVERROR_EOF,  // End of file
                Ok(bytes_read) => bytes_read as i32,    // Return the number of bytes read
                Err(_) => ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO), // I/O error
            }
        })
    };

    // Define the seek callback for custom input handling
    let seek_callback: Box<dyn FnMut(i64, i32) -> i64> = {
        let input = Arc::clone(&input_file);
        Box::new(move |offset: i64, whence: i32| -> i64 {
            let mut input = input.lock().unwrap();

            // Handle AVSEEK_SIZE to return the total size of the file
            if whence == ffmpeg_sys_next::AVSEEK_SIZE {
                if let Ok(size) = input.metadata().map(|m| m.len() as i64) {
                    println!("FFmpeg requested stream size: {}", size);
                    return size;  // Return the total file size
                }
                return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
            }

            // Handle seeking in various modes
            let seek_result = match whence {
                ffmpeg_sys_next::SEEK_SET => input.seek(SeekFrom::Start(offset as u64)),
                ffmpeg_sys_next::SEEK_CUR => input.seek(SeekFrom::Current(offset)),
                ffmpeg_sys_next::SEEK_END => input.seek(SeekFrom::End(offset)),
                _ => {
                    println!("Unsupported seek mode: {whence}");
                    return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE) as i64;
                }
            };

            // Return the new position or error code
            match seek_result {
                Ok(new_pos) => new_pos as i64,
                Err(_) => ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64,
            }
        })
    };

    // Create an input stream with the custom read and seek callbacks
    let mut input: Input = read_callback.into();
    input = input.set_seek_callback(seek_callback);

    // Open the output file in a thread-safe manner
    let output_file = Arc::new(Mutex::new(
        File::create(output_file).expect("Failed to create output file"),
    ));

    // Define the write callback for custom output handling
    let write_callback: Box<dyn FnMut(&[u8]) -> i32> = {
        let output_file = Arc::clone(&output_file);
        Box::new(move |buf: &[u8]| -> i32 {
            let mut output_file = output_file.lock().unwrap();
            match output_file.write_all(buf) {
                Ok(_) => buf.len() as i32,  // Return the number of bytes written
                Err(_) => ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO), // I/O error
            }
        })
    };

    // Define the seek callback for the output file
    let seek_callback: Box<dyn FnMut(i64, i32) -> i64> = {
        let output_file = Arc::clone(&output_file);
        Box::new(move |offset: i64, whence: i32| -> i64 {
            let mut file = output_file.lock().unwrap();

            match whence {
                // Handle AVSEEK_SIZE to return the total size of the file
                ffmpeg_sys_next::AVSEEK_SIZE => {
                    if let Ok(size) = file.metadata().map(|m| m.len() as i64) {
                        println!("FFmpeg requested stream size: {size}");
                        return size;  // Return the total file size
                    }
                    return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
                }

                // Handle seeking based on byte offset
                ffmpeg_sys_next::AVSEEK_FLAG_BYTE => {
                    println!("FFmpeg requested byte-based seeking. Seeking to byte offset: {offset}");
                    if let Ok(new_pos) = file.seek(SeekFrom::Start(offset as u64)) {
                        return new_pos as i64;  // Return the new position
                    }
                    return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
                }

                // Handle standard seek modes
                ffmpeg_sys_next::SEEK_SET => file.seek(SeekFrom::Start(offset as u64)),
                ffmpeg_sys_next::SEEK_CUR => file.seek(SeekFrom::Current(offset)),
                ffmpeg_sys_next::SEEK_END => file.seek(SeekFrom::End(offset)),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Unsupported seek mode",
                )),
            }
                .map_or(
                    ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64,
                    |pos| pos as i64,
                )
        })
    };

    // Create an output stream with the custom write and seek callbacks
    let mut output: Output = write_callback.into();
    output = output.set_seek_callback(seek_callback)
        .set_format("mp4");

    // Initialize and run the FFmpeg context with the custom input/output
    FfmpegContext::builder()
        .input(input)
        .output(output)
        .build().unwrap()
        .start().unwrap()
        .wait().unwrap();
}
