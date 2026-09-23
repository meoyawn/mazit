fn main() {
    println!("cargo:rerun-if-changed=assets/Info.plist");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
        && std::env::var_os("CARGO_FEATURE_DESKTOP").is_some()
    {
        // Make AppKit honor the AutoFill opt-out when running outside the .app bundle.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!(
            "cargo:rustc-link-arg-bin=mazit=-Wl,-sectcreate,__TEXT,__info_plist,{manifest_dir}/assets/Info.plist"
        );
    }
}
