fn main() {
    println!("cargo:rerun-if-changed=assets/scrp.rc");
    println!("cargo:rerun-if-changed=assets/scrp.ico");
    embed_resource::compile_for(
        "assets/scrp.rc",
        ["star-citizen-rich-presence"],
        embed_resource::NONE,
    )
    .manifest_required()
    .expect("could not embed the Windows application icon");
}
