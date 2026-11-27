fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let plist = format!("{manifest}/../../packaging/Info.plist");
        println!("cargo:rerun-if-changed={plist}");
        println!("cargo:rustc-link-arg-bin=surveillance=-Wl,-sectcreate,__TEXT,__info_plist,{plist}");
    }
}
