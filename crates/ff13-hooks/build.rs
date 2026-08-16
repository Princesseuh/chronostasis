fn main() {
    // On the Windows target, hand the linker our export definition so the proxy
    // exports `Direct3DCreate9` etc. undecorated at the right ordinals.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("d3d9.def");
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
        println!("cargo:rustc-cdylib-link-arg=-Wl,--enable-stdcall-fixup");
        println!("cargo:rerun-if-changed=d3d9.def");
    }
}
