pub mod build;
pub mod depfile;
pub mod deps_log;
pub mod dyndep;
pub mod manifest;

pub fn program_name() -> &'static str {
    static NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let invoked_as_ninja = std::env::args_os().next().is_some_and(|arg0| {
            std::path::Path::new(&arg0)
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case("ninja"))
        });
        if invoked_as_ninja { "ninja" } else { "knight" }
    })
}

pub use build::{
    BuildOptions, BuildOutcome, apply_dyndep_files, ensure_process_tree_cleanup,
    install_interrupt_handler, last_build_exit_code, manifest_with_existing_dyndeps,
    render_unescaped_binding, resolve_target_path, run_build,
};
pub use manifest::{
    Diagnostic, Edge, Manifest, Pool, Rule, canonicalize_owned_path, canonicalize_path,
    load_manifest, parse_manifest, spellcheck, unknown_target_message,
};
