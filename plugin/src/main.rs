//! Stem Split as a standalone app (no DAW): drop a file, mix the stems, drag them out.
//! Double-clicked (no arguments) it uses the default audio output; the usual NIH-plug options
//! (--backend, --output-device, --sample-rate, --period-size …) still work from a terminal.

fn main() {
    // STEM_SPLIT_PLUGIN_UI=1 shows the plugin's UI instead (for testing it without a DAW)
    stem_split_plugin::set_app_mode(std::env::var_os("STEM_SPLIT_PLUGIN_UI").is_none());
    let mut args: Vec<String> = std::env::args().collect();
    // Finder passes -psn_… on older macOS; drop anything the option parser doesn't know.
    args.retain(|a| !a.starts_with("-psn_"));
    if args.len() <= 1 {
        args.extend(["--period-size".into(), "1024".into()]);
    }
    nih_plug::prelude::nih_export_standalone_with_args::<stem_split_plugin::StemSplit, _>(args);
}
