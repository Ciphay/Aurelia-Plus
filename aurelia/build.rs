fn main() {
    for (proto, out) in [
        ("proto/service_cloudconfigstore.proto", "cloudconfig"),
        ("proto/service_storequery.proto", "storequery"),
        ("proto/service_wishlist.proto", "wishlist"),
    ] {
        println!("cargo:rerun-if-changed={proto}");
        protobuf_codegen::Codegen::new()
            .pure()
            .include("proto")
            .input(proto)
            .cargo_out_dir(out)
            .run_from_script();
    }
    embed_plus_shim();
}

fn embed_plus_shim() {
    println!("cargo::rustc-check-cfg=cfg(aurelia_plus_shim)");
    println!("cargo:rerun-if-env-changed=AURELIA_PLUS_SHIM_DIR");
    let dir = std::env::var_os("AURELIA_PLUS_SHIM_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plus/run/shimsteam/legacycompat"));
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("plus_shim");
    std::fs::create_dir_all(&out).expect("plus_shim dir");
    let names = ["steamclient64.dll", "steamclient.dll", "windows.ui.core.textinput.dll", "windows.ui.core.textinput32.dll", "Steam.exe"];
    for name in names {
        println!("cargo:rerun-if-changed={}", dir.join(name).display());
    }
    if names.iter().all(|n| dir.join(n).is_file()) {
        for name in names {
            std::fs::copy(dir.join(name), out.join(name)).expect("copy shim dll");
        }
        println!("cargo:rustc-cfg=aurelia_plus_shim");
    } else {
        println!("cargo:warning=Aurelia Plus shim not found in {} (run plus/prepare.sh); building without `play --plus`", dir.display());
    }
}
