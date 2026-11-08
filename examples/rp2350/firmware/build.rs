use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../..");
    let image = env::var_os("PCMON_DISK_IMAGE")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("output/examples/rp2350/host-tools.img"));
    let image = image
        .canonicalize()
        .expect("Build host tools and run examples/rp2350/tools/make_disk.py first");
    let size = fs::metadata(&image).unwrap().len();
    assert!(
        size > 0 && size.is_multiple_of(512) && size <= 3 * 1024 * 1024,
        "invalid disk image size"
    );
    println!("cargo:rustc-env=PCMON_DISK_IMAGE={}", image.display());
    println!("cargo:rerun-if-env-changed=PCMON_DISK_IMAGE");
    println!("cargo:rerun-if-changed={}", image.display());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
}
