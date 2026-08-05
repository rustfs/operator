fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Prefer PATH `protoc` (Dockerfile installs protobuf-compiler); fall back to vendored.
    if std::env::var_os("PROTOC").is_none()
        && let Ok(protoc) = protoc_bin_vendored::protoc_bin_path()
    {
        // SAFETY: build script is single-threaded before codegen.
        unsafe {
            std::env::set_var("PROTOC", protoc);
        }
    }
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["proto/cosi.proto"], &["proto"])?;
    Ok(())
}
