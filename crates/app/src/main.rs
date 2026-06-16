//! ShaderBuilder application entry point.
//!
//! Phase 0: a plain binary that links every workspace crate, proving the whole
//! dependency graph (`app` → all of Architecture §B) compiles end to end. The
//! Tauri window + React/React Flow frontend land in #11; the `tauri::ipc::Channel`
//! binary frame path in #13.
fn main() {
    let crates = [
        core_model::NAME,
        ir::NAME,
        codegen_slang::NAME,
        codegen_glslp::NAME,
        slang_compile::NAME,
        preview_engine::NAME,
        source::NAME,
        preset_io::NAME,
    ];
    println!("ShaderBuilder workspace linked: {}", crates.join(", "));
}
