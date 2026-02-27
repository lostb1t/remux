use ez_ffmpeg::codec::{get_decoders, get_encoders};

fn main() {
    // Fetch and print all available encoders
    // Encoders are used to convert data into different formats (e.g., encoding video, audio).
    get_encoders().iter().for_each(|x| println!("{:?}", x));

    // Fetch and print all available decoders
    // Decoders are used to decode the input data (e.g., decoding video, audio).
    get_decoders().iter().for_each(|x| println!("{:?}", x));
}
