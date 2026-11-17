fn main() {
    println!("cargo:rerun-if-changed=assets/traykit.rc");
    println!("cargo:rerun-if-changed=assets/traykit.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/traykit.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
