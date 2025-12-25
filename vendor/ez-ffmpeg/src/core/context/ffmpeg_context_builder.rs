use crate::core::context::input::Input;
use crate::core::context::output::Output;
use crate::core::context::ffmpeg_context::FfmpegContext;
use crate::core::context::filter_complex::FilterComplex;

/// A builder for constructing [`FfmpegContext`] objects with customized inputs,
/// outputs, and filter configurations. Typically, you will start by calling
/// [`FfmpegContext::builder()`], then chain methods to add inputs, outputs, or
/// filter descriptions, and finally invoke [`build()`](FfmpegContextBuilder::build) to produce an `FfmpegContext`.
///
/// # Examples
///
/// ```rust
/// // 1. Create a builder
/// let builder = FfmpegContext::builder();
///
/// // 2. Add at least one input and one output
/// let ffmpeg_context = builder
///     .input("input.mp4")
///     // 3. Optionally add filters and more inputs/outputs
///     .filter_desc("hue=s=0") // Example FFmpeg filter
///     .output("output.mp4")
///     .build()
///     .expect("Failed to build FfmpegContext");
///
/// // 4. Use ffmpeg_context with FfmpegScheduler (e.g., `ffmpeg_context.start()`).
/// ```
#[must_use]
pub struct FfmpegContextBuilder {
    independent_readrate: bool,
    inputs: Vec<Input>,
    filter_descs: Vec<FilterComplex>,
    outputs: Vec<Output>,
    copy_ts: bool,
}

impl FfmpegContextBuilder {

    /// Creates a new, empty `FfmpegContextBuilder`. Generally, you won't call this
    /// directly; instead, use [`FfmpegContext::builder()`] as your entry point.
    ///
    /// # Example
    ///
    /// ```rust
    /// let builder = FfmpegContextBuilder::new();
    /// ```
    pub fn new() -> Self {
        Self {
            independent_readrate: false,
            inputs: vec![],
            filter_descs: vec![],
            outputs: vec![],
            copy_ts: false,
        }
    }

    /// Enables independent read rate control for multiple inputs, specifically addressing issues
    /// with sequential processing filters like 'concat'.
    ///
    /// # Core Problem Solved
    ///
    /// When processing multiple inputs sequentially with filters like 'concat', FFmpeg's default
    /// read rate mechanism causes unintended behavior:
    ///
    /// 1. By default, FFmpeg initializes a single 'wallclock_start' timestamp at the beginning
    ///    of processing, which serves as the reference for calculating read speeds.
    ///
    /// 2. In sequential processing (like with concat filter), inputs are processed one after another:
    ///    - The first input starts immediately and is read at the specified rate
    ///    - Subsequent inputs remain locked until previous inputs finish processing
    ///
    /// 3. When later inputs are unlocked (which could be minutes or hours later), their read rate
    ///    is calculated using the original 'wallclock_start' time, causing them to be read far too
    ///    quickly - often at maximum speed regardless of the set readrate.
    ///
    /// 4. This rapid reading loads large amounts of data into memory too quickly, potentially
    ///    causing out-of-memory errors with large media files.
    ///
    /// # How This Fix Works
    ///
    /// When `independent_readrate()` is enabled:
    /// - Each input gets its own effective 'wallclock_start' reference time when it begins processing
    /// - This ensures each input maintains the specified read rate, regardless of when in the
    ///   sequence it's processed
    /// - Memory usage becomes more consistent and predictable throughout the entire processing pipeline
    ///
    /// # Practical Example
    ///
    /// ```rust
    /// let result = FfmpegContext::builder()
    ///     .independent_readrate() // Enable independent read rates
    ///     .input(Input::from("file1.mp4").set_readrate(1.0)) // First input at 1x speed
    ///     .input(Input::from("file2.mp4").set_readrate(1.0)) // Second input also at 1x speed
    ///     .input(Input::from("file3.mp4").set_readrate(1.0)) // Third input also at 1x speed
    ///     .filter_desc("[0:v][0:a][1:v][1:a][2:v][2:a]concat=n=3:v=1:a=1") // Concatenate all inputs
    ///     .output(output)
    ///     .build()
    ///     .unwrap();
    /// ```
    ///
    /// In this example, without `independent_readrate()`, the second and third files would be read
    /// much faster than intended after the first file completes. With this option enabled, each file
    /// maintains its 1.0x read rate precisely when it begins processing.
    ///
    /// # When To Use
    ///
    /// This option is essential when:
    /// - Using the 'concat' filter with multiple inputs
    /// - Processing long-duration media files sequentially
    /// - Setting specific read rates for inputs (via `set_readrate()`)
    /// - Memory usage is a concern in your application
    pub fn independent_readrate(mut self) -> Self {
        self.independent_readrate = true;
        self
    }

    /// Adds a single [`Input`] to the builder. This can be a file path, a URL,
    /// or a custom input with callbacks.
    ///
    /// Calling this method multiple times adds multiple distinct inputs.
    ///
    /// # Parameters
    /// - `input` - Anything convertible into an [`Input`], such as a `&str`, `String`,
    ///   or a custom callback-based `Input`.
    ///
    /// # Returns
    /// A modified `FfmpegContextBuilder`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let context = FfmpegContextBuilder::new()
    ///     .input("video.mp4")
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn input(mut self, input: impl Into<Input>) -> Self {
        self.inputs.push(input.into());
        self
    }

    /// Replaces the current list of inputs with a new collection.
    ///
    /// This method takes a `Vec` of items convertible into [`Input`]s and sets them
    /// as the complete set of inputs for the builder. Any previously added inputs
    /// will be discarded.
    ///
    /// # Parameters
    /// - `inputs` - A vector of items (e.g. `&str`, `String`, custom callbacks)
    ///   that will be converted into `Input`s.
    ///
    /// # Returns
    /// A modified `FfmpegContextBuilder`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let inputs = vec!["input1.mp4", "input2.mp4"];
    /// let context = FfmpegContextBuilder::new()
    ///     .inputs(inputs)
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn inputs(mut self, inputs: Vec<impl Into<Input>>) -> Self {
        self.inputs = inputs.into_iter().map(|input| input.into()).collect();
        self
    }

    /// Adds a single [`Output`] to the builder, representing a single output
    /// destination (file path, URL, or custom write callback).
    ///
    /// Calling this multiple times adds multiple outputs (e.g., for transcoding
    /// to different formats simultaneously).
    ///
    /// # Parameters
    /// - `output` - Anything convertible into an [`Output`], such as a `&str`, `String`,
    ///   or a callback-based output.
    ///
    /// # Returns
    /// A modified `FfmpegContextBuilder`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let context = FfmpegContextBuilder::new()
    ///     .output("output.mp4")
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn output(mut self, output: impl Into<Output>) -> Self {
        self.outputs.push(output.into());
        self
    }

    /// Replaces the current list of outputs with a new collection.
    ///
    /// This method takes a `Vec` of items convertible into [`Output`] and sets them
    /// as the complete set of outputs for the builder. Any previously added outputs
    /// will be discarded.
    ///
    /// # Parameters
    /// - `outputs` - A vector of items (e.g. `&str`, `String`, or custom callback-based
    ///   outputs) that will be converted into `Output`s.
    ///
    /// # Returns
    /// A modified `FfmpegContextBuilder`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let outputs = vec!["output1.mp4", "output2.mkv"];
    /// let context = FfmpegContextBuilder::new()
    ///     .outputs(outputs)
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn outputs(mut self, outputs: Vec<impl Into<Output>>) -> Self {
        self.outputs = outputs.into_iter().map(|output| output.into()).collect();
        self
    }

    /// Adds a single filter description (e.g., `"hue=s=0"`, `"scale=1280:720"`)
    /// that applies to one or more inputs. Each filter description can also
    /// contain complex filter graphs.
    ///
    /// Internally, it's converted into a [`FilterComplex`] object. These filters
    /// can further manipulate or route media streams before they reach the outputs.
    ///
    /// # Parameters
    /// - `filter_desc` - A string or [`FilterComplex`] describing filter operations
    ///   in FFmpeg's filter syntax.
    ///
    /// # Returns
    /// A modified `FfmpegContextBuilder`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let context = FfmpegContextBuilder::new()
    ///     .input("input.mp4")
    ///     .filter_desc("hue=s=0") // Desaturate the video
    ///     .output("output_gray.mp4")
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn filter_desc(mut self, filter_desc: impl Into<FilterComplex>) -> Self {
        self.filter_descs.push(filter_desc.into());
        self
    }

    /// Replaces the current filter descriptions with a new list of them.
    ///
    /// This method takes a `Vec` of items convertible into [`FilterComplex`], allowing
    /// you to specify multiple complex filter graphs or distinct filter operations
    /// all at once. Any previously added filters will be discarded.
    ///
    /// # Parameters
    /// - `filter_descs` - A vector of strings or [`FilterComplex`] objects that define
    ///   FFmpeg filter operations.
    ///
    /// # Returns
    /// A modified `FfmpegContextBuilder`, allowing method chaining.
    ///
    /// # Example
    /// ```rust
    /// let filter_chains = vec!["scale=1280:720", "drawtext=fontfile=...:text='Watermark'"];
    /// let context = FfmpegContextBuilder::new()
    ///     .input("input.mp4")
    ///     .filter_descs(filter_chains)
    ///     .output("output_scaled.mp4")
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn filter_descs(mut self, filter_descs: Vec<impl Into<FilterComplex>>) -> Self {
        self.filter_descs = filter_descs.into_iter().map(|filter| filter.into()).collect();
        self
    }

    /// Enables timestamp copying from input to output
    /// 
    /// This method sets the `copy_ts` flag to true, which is equivalent to FFmpeg's `-copyts` option.
    /// When enabled, timestamps from the input stream are preserved in the output stream without modification.
    /// This is useful when you want to maintain the original timing information from the source media.
    /// 
    /// # Example
    /// ```
    /// let builder = FfmpegContextBuilder::new()
    ///     .copyts();
    /// ```
    pub fn copyts(mut self) -> Self {
        self.copy_ts = true;
        self
    }

    /// Finalizes this builder, creating an [`FfmpegContext`] which can then be used
    /// to run FFmpeg jobs via [`FfmpegContext::start()`](FfmpegContext::start) or by constructing an
    /// [`FfmpegScheduler`](crate::FfmpegScheduler) yourself.
    ///
    /// # Errors
    /// Returns an error if any configuration issues are found (e.g., invalid URL syntax,
    /// conflicting filter settings, etc.).
    /// (The actual validation depends on how [`FfmpegContext::new_with_independent_readrate`](FfmpegContext::new_with_independent_readrate) is implemented.)
    ///
    /// # Example
    /// ```rust
    /// let context = FfmpegContextBuilder::new()
    ///     .input("input1.mp4")
    ///     .input("input2.mp4")
    ///     .output("combined_output.mkv")
    ///     .build()
    ///     .expect("Failed to build FfmpegContext");
    ///
    /// // Use the context to start FFmpeg processing
    /// let scheduler = context.start().expect("Failed to start FFmpeg job");
    /// scheduler.wait().unwrap();
    /// ```
    pub fn build(self) -> crate::error::Result<FfmpegContext> {
        FfmpegContext::new_with_options(
            self.independent_readrate,
            self.inputs,
            self.filter_descs,
            self.outputs,
            self.copy_ts
        )
    }
}
