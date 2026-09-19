// Keep the dependency-free build helper's fixtures in Cargo's test discovery.
// They can also run directly with rustc without compiling the GPUI graph.
#[path = "../build_version.rs"]
mod build_version;
