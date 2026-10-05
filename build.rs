// The .exe's icon and manifest (assets/installer.rc). The manifest says
// asInvoker: the installer runs as you and asks Windows for permission only
// when a step needs it (obs-browser lives in Program Files).
fn main() {
    println!("cargo:rerun-if-changed=assets/installer.rc");
    println!("cargo:rerun-if-changed=assets/installer.ico");
    println!("cargo:rerun-if-changed=assets/installer.manifest");
    embed_resource::compile("assets/installer.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
