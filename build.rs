fn main() {
    println!("cargo:rerun-if-changed=windows/knight.manifest");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none()
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let manifest = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("windows/knight.manifest");
    println!("cargo:rustc-link-arg-bin=knight=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=knight=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
