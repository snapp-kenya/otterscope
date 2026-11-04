use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dist = manifest.join("../../../web/dist");
    if !dist.exists() {
        std::fs::create_dir_all(&dist).expect("create web/dist");
        std::fs::write(
            dist.join("index.html"),
            "<!doctype html><html><head><title>Otterscope</title></head><body><p>UI not built. Run `bun run build` in web/.</p></body></html>",
        )
        .expect("write placeholder");
    }
    println!("cargo:rerun-if-changed={}", dist.display());

    // The container build passes the git SHA as `--build-arg version`, which
    // the dockerfile exports as OTTERSCOPE_VERSION. A plain cargo build leaves
    // it unset and the crate version is reported instead.
    println!("cargo:rerun-if-env-changed=OTTERSCOPE_VERSION");
    if let Ok(version) = std::env::var("OTTERSCOPE_VERSION") {
        let version = version.trim();
        if !version.is_empty() {
            println!("cargo:rustc-env=OTTERSCOPE_VERSION={version}");
        }
    }
}
