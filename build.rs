//! Compiles the dialog templates, manifest and version resource into the exe.
//! `embed-resource` runs rc.exe at build time; nothing of it ships.
fn main() {
    embed_resource::compile("videoscribe.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
    println!("cargo:rerun-if-changed=videoscribe.rc");
    println!("cargo:rerun-if-changed=videoscribe.manifest");
}
