use std::env;
use std::path::PathBuf;

type EmptyResult = Result<(), GenericError>;
type GenericError = Box<dyn ::std::error::Error + Send + Sync>;

fn main() -> EmptyResult {
    println!("cargo:rerun-if-changed=bindings.h");

    // Requires clang and libclang-dev packages
    let bindings = bindgen::Builder::default()
        .header("bindings.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .derive_default(true)
        .generate()
        .map_err(|e| format!("Failed to generate bindings: {e}"))?;

    let out_dir = env::var("OUT_DIR").map_err(|_|
        "OUT_DIR environment variable is missing")?;

    let out_path = PathBuf::from(out_dir).join("bindings.rs");
    bindings.write_to_file(&out_path).map_err(|e| format!(
        "Failed to write generated bindings to {out_path:?}: {e}"))?;

    Ok(())
}