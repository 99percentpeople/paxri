fn main() {
    println!("cargo:rerun-if-changed=windows/app.rc");
    println!("cargo:rerun-if-changed=windows/app.manifest");
    embed_resource::compile("windows/app.rc", embed_resource::NONE)
        .manifest_required()
        .expect("embed the Windows desktop manifest");
}
