fn main() {
    println!("cargo::rerun-if-changed=src/lib.rs");

    cbindgen::Builder::new()
        .with_crate(env!("CARGO_MANIFEST_DIR"))
        .with_language(cbindgen::Language::C)
        .with_cpp_compat(true)
        .with_include_version(true)
        .with_include_guard("CHEETAH_RTMP_H")
        .with_no_includes()
        .with_sys_include("stdbool.h")
        .with_sys_include("stddef.h")
        .with_sys_include("stdint.h")
        .generate()
        .expect("failed to generate C bindings")
        .write_to_file("include/cheetah_rtmp.h");
}
