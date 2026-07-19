use std::fs;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let cargo_toml = fs::read_to_string(format!("{manifest_dir}/Cargo.toml")).unwrap_or_default();

    let rev = cargo_toml
        .lines()
        .filter(|line| line.contains("avcodec"))
        .find_map(|line| {
            line.split("rev")
                .nth(1)
                .and_then(|s| s.split('"').nth(1))
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=AVCODEC_REVISION={rev}");
}
