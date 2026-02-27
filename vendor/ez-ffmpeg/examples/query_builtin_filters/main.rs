use ez_ffmpeg::filter::get_filters;

fn main() {
    // Retrieve and print the list of built-in FFmpeg filters
    // `get_filters()` fetches all the available filters in FFmpeg
    get_filters().iter().for_each(|filter| println!("{:?}", filter));
}
