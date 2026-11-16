fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/riva/proto/riva_asr.proto");
    println!("cargo:rerun-if-changed=proto/riva/proto/riva_audio.proto");
    println!("cargo:rerun-if-changed=proto/riva/proto/riva_common.proto");
    println!("cargo:rerun-if-changed=build.rs");

    let protoc = protoc_bin_vendored::protoc_bin_path()
        .expect("vendored protoc must be present (protoc-bin-vendored crate)");
    std::env::set_var("PROTOC", &protoc);

    tonic_prost_build::configure()
        .build_server(false)
        .compile_protos(
            &[
                "proto/riva/proto/riva_asr.proto",
                "proto/riva/proto/riva_audio.proto",
                "proto/riva/proto/riva_common.proto",
            ],
            &["proto"],
        )
        .map_err(Into::into)
}
