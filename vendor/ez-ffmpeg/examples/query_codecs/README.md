# ez-ffmpeg Example: Query Codecs

This example demonstrates how to query the available codecs (encoders and decoders) that FFmpeg can use. It utilizes the `get_encoders` and `get_decoders` functions from the `ez-ffmpeg` crate to list all supported encoding and decoding formats.

## Overview

- **Encoders**: These are used to convert data from one format to another, such as encoding video or audio streams into a specified format.
- **Decoders**: These are used to decode encoded streams back to raw data, such as decoding video or audio streams into usable frames or samples.

The example will print the list of available encoders and decoders, helping you understand the codecs supported by FFmpeg in your environment.
