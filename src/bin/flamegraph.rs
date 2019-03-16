use env_logger::Env;
use std::io;
use std::path::{Path, PathBuf};
use structopt::StructOpt;

use inferno::flamegraph::{
    self, color::BackgroundColor, color::PaletteMap, Direction, FuncFrameAttrsMap, Options,
    Palette, DEFAULT_TITLE,
};

#[derive(Debug, StructOpt)]
#[structopt(
    name = "inferno-flamegraph",
    author = "",
    about = r#"
This takes stack samples and renders a call graph, allowing hot functions
and codepaths to be quickly identified. Stack samples can be generated using
tools such as DTrace, perf, SystemTap, and Instruments.
"#,
    after_help = r#"
 USAGE: inferno-flamegraph [options] input.txt > graph.svg

        grep funcA input.txt | inferno-flamegraph [options] > graph.svg

 Then open the resulting .svg in a web browser, for interactivity: mouse-over
 frames for info, click to zoom, and ctrl-F to search.

 The input is stack frames and sample counts formatted as single lines.  Each
 frame in the stack is semicolon separated, with a space and count at the end
 of the line.  These can be generated for Linux perf script output using
 stackcollapse-perf.pl, for DTrace using stackcollapse.pl, and for other tools
 using the other stackcollapse programs.  Example input:

  swapper;start_kernel;rest_init;cpu_idle;default_idle;native_safe_halt 1

 An optional extra column of counts can be provided to generate a differential
 flame graph of the counts, colored red for more, and blue for less.  This
 can be useful when using flame graphs for non-regression testing.
 See the header comment in the difffolded.pl program for instructions.

 The input functions can optionally have annotations at the end of each
 function name, following a precedent by some tools (Linux perf's _[k]):
     _[k] for kernel
 _[i] for inlined
 _[j] for jit
 _[w] for waker
 Some of the stackcollapse programs support adding these annotations, eg,
 stackcollapse-perf.pl --kernel --jit. They are used merely for colors by
 some palettes, eg, flamegraph.pl --color=java.

 The output flame graph shows relative presence of functions in stack samples.
 The ordering on the x-axis has no meaning; since the data is samples, time
 order of events is not known.  The order used sorts function names
 alphabetically.

 While intended to process stack samples, this can also process stack traces.
 For example, tracing stacks for memory allocation, or resource usage.  You
 can use --title to set the title to reflect the content, and --countname
 to change "samples" to "bytes" etc.

 There are a few different palettes, selectable using --color.  By default,
 the colors are selected at random (except for differentials).  Functions
 called "-" will be printed gray, which can be used for stack separators (eg,
 between user and kernel stacks).

 HISTORY

 This was inspired by Neelakanth Nadgir's excellent function_call_graph.rb
 program, which visualized function entry and return trace events.  As Neel
 wrote: "The output displayed is inspired by Roch's CallStackAnalyzer which
 was in turn inspired by the work on vftrace by Jan Boerhout".  See:
 https://blogs.oracle.com/realneel/entry/visualizing_callstacks_via_dtrace_and

 Copyright 2016 Netflix, Inc.
 Copyright 2011 Joyent, Inc.  All rights reserved.
 Copyright 2011 Brendan Gregg.  All rights reserved.
"#
)]
struct Opt {
    /// File containing attributes to use for the SVG frames of particular functions.
    /// Each line in the file should be a function name followed by a tab,
    /// then a sequence of tab separated name=value pairs.
    #[structopt(long = "nameattr")]
    nameattr_file: Option<PathBuf>,

    /// Plot the flame graph up-side-down.
    #[structopt(short = "i", long = "inverted")]
    inverted: bool,

    /// Collapsed perf output files. With no INFILE, or INFILE is -, read STDIN.
    #[structopt(name = "INFILE", parse(from_os_str))]
    infiles: Vec<PathBuf>,

    /// Set color palette
    #[structopt(
        short = "c",
        long = "colors",
        default_value = "hot",
        raw(
            possible_values = r#"&["hot","mem","io","wakeup","java","js","perl","red","green","blue","aqua","yellow","purple","orange"]"#
        )
    )]
    colors: Palette,

    /// Set background colors. Gradient choices are yellow (default), blue, green, grey; flat colors use "#rrggbb"
    #[structopt(long = "bgcolors")]
    bgcolors: Option<BackgroundColor>,

    /// Colors are keyed by function name hash
    #[structopt(long = "hash")]
    hash: bool,

    /// Use consistent palette (palette.map)
    #[structopt(long = "cp")]
    cp: bool,

    /// Change title text
    #[structopt(long = "title", default_value = "Flame Graph")]
    title: String,

    /// Second level title (optional)
    #[structopt(long = "subtitle")]
    subtitle: Option<String>,

    /// Width of image
    #[structopt(long = "width", default_value = "1200")]
    image_width: usize,

    /// Height of each frame
    #[structopt(long = "height", default_value = "16")]
    frame_height: usize,

    /// Omit smaller functions (default 0.1 pixels)
    #[structopt(long = "minwidth", default_value = "0.1")]
    min_width: f64,

    /// Font type
    #[structopt(long = "fonttype", default_value = "Verdana")]
    font_type: String,

    /// Font size
    #[structopt(long = "fontsize", default_value = "12")]
    font_size: usize,

    /// Font width
    #[structopt(long = "fontwidth", default_value = "0.59")]
    font_width: f64,

    /// Count type label
    #[structopt(long = "countname", default_value = "samples")]
    count_name: String,

    /// Name type label
    #[structopt(long = "nametype", default_value = "Function:")]
    name_type: String,

    /// Set embedded notes in SVG
    #[structopt(long = "notes", default_value = "")]
    notes: String,

    /// Switch differential hues (green<->red)
    #[structopt(long = "negate")]
    negate: bool,

    /// Factor to scale sample counts by
    #[structopt(long = "factor", default_value = "1.0")]
    factor: f64,

    /// Silence all log output
    #[structopt(short = "q", long = "quiet")]
    quiet: bool,

    /// Verbose logging mode (-v, -vv, -vvv)
    #[structopt(short = "v", long = "verbose", parse(from_occurrences))]
    verbose: usize,

    /// Pretty print XML with newlines and indentation.
    #[structopt(long = "pretty-xml")]
    pretty_xml: bool,

    /// Don't include static JavaScript in flame graph.
    /// This flag is hidden since it's only meant to be used in
    /// tests so we don't have to include the same static
    /// JavaScript in all of the test files.
    #[structopt(raw(hidden = "true"), long = "no-javascript")]
    no_javascript: bool,
}

impl<'a> Opt {
    fn into_parts(self) -> (Vec<PathBuf>, Options<'a>) {
        let mut options = Options::default();
        options.colors = self.colors;
        options.bgcolors = self.bgcolors;
        options.hash = self.hash;
        if let Some(file) = self.nameattr_file {
            match FuncFrameAttrsMap::from_file(&file) {
                Ok(m) => {
                    options.func_frameattrs = m;
                }
                Err(e) => panic!("Error reading {}: {:?}", file.display(), e),
            }
        };
        if self.inverted {
            options.direction = Direction::Inverted;
            if self.title == DEFAULT_TITLE {
                options.title = "Icicle Graph".to_string();
            }
        }
        options.negate_differentials = self.negate;
        options.factor = self.factor;
        options.pretty_xml = self.pretty_xml;
        options.no_javascript = self.no_javascript;

        // set style options
        options.subtitle = self.subtitle;
        options.image_width = self.image_width;
        options.frame_height = self.frame_height;
        options.min_width = self.min_width;
        options.font_type = self.font_type;
        options.font_size = self.font_size;
        options.font_width = self.font_width;
        options.count_name = self.count_name;
        options.name_type = self.name_type;
        options.notes = self.notes;
        options.negate_differentials = self.negate;
        options.factor = self.factor;
        (self.infiles, options)
    }
}

const PALETTE_MAP_FILE: &str = "palette.map"; // default name for the palette map file

fn main() -> quick_xml::Result<()> {
    let opt = Opt::from_args();

    // Initialize logger
    if !opt.quiet {
        env_logger::Builder::from_env(Env::default().default_filter_or(match opt.verbose {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        }))
        .default_format_timestamp(false)
        .init();
    }

    let mut palette_map = match fetch_consistent_palette_if_needed(opt.cp, PALETTE_MAP_FILE) {
        Ok(palette_map) => palette_map,
        Err(e) => panic!("Error reading {}: {:?}", PALETTE_MAP_FILE, e),
    };

    let (infiles, mut options) = opt.into_parts();
    options.palette_map = palette_map.as_mut();

    flamegraph::from_files(&mut options, &infiles, io::stdout().lock())?;
    save_consistent_palette_if_needed(&palette_map, PALETTE_MAP_FILE).map_err(quick_xml::Error::Io)
}

fn fetch_consistent_palette_if_needed(
    use_consistent_palette: bool,
    palette_file: &str,
) -> io::Result<Option<PaletteMap>> {
    let palette_map = if use_consistent_palette {
        let path = Path::new(palette_file);
        Some(PaletteMap::load_from_file_or_empty(&path)?)
    } else {
        None
    };

    Ok(palette_map)
}

fn save_consistent_palette_if_needed(
    palette_map: &Option<PaletteMap>,
    palette_file: &str,
) -> io::Result<()> {
    if let Some(palette_map) = palette_map {
        let path = Path::new(palette_file);
        palette_map.save_to_file(&path)?;
    }

    Ok(())
}
