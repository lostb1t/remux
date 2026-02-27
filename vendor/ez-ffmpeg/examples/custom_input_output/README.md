# ez-ffmpeg Example: Custom Input/Output

This example demonstrates how to use `ez-ffmpeg` to handle custom input and output streams. The goal is to allow FFmpeg to read data from and write data to sources that aren't traditional files, such as in-memory buffers or custom protocols.

## Use Cases

### 1. **Custom Input**:
This can be used when the data source isn't a standard file but something like:
- **Network streams**: Custom network protocols or APIs that stream media data.
- **In-memory buffers**: When the data is preloaded in memory and needs to be passed directly to FFmpeg for processing.
- **Device inputs**: If you want to read media from devices, such as webcams or custom hardware.

### 2. **Custom Output**:
This can be used when you want to:
- **Stream to a server**: Custom protocols for writing output to a server.
- **Store data in-memory**: Useful for scenarios where the output isn't saved to disk but needs to be passed along to another component in the program.

### 3. **Custom Seek Behavior**:
Handling non-standard seek operations, like seeking in memory buffers, handling seek in live streams, or responding to specific seek commands in network protocols.

## Key Features

- **Custom Read Callback**: Allows you to provide a function for reading data, enabling non-standard input sources.
- **Custom Seek Callback**: Lets you define how seeking should work for custom data sources, including handling `AVSEEK_SIZE` and other seek modes.
- **Custom Write Callback**: Allows you to provide a function for writing data, useful for writing to non-file destinations.

## Code Walkthrough

1. **Custom Input Handling**:
   - Open the input file using `File::open` wrapped in an `Arc<Mutex<>>` for thread-safety.
   - Define a `read_callback` function to supply data to FFmpeg. This function reads from the file and returns the number of bytes read.
   - Define a `seek_callback` to handle seek requests from FFmpeg.

2. **Custom Output Handling**:
   - Open the output file using `File::create` wrapped in `Arc<Mutex<>>`.
   - Define a `write_callback` to handle writing data to the output file.
   - Define a `seek_callback` for the output to handle seeking and returning the new position.

3. **FFmpeg Context**:
   - Create a custom `Input` and `Output` object using the callbacks.
   - Use `FfmpegContext::builder()` to configure the input and output, then run the process.
