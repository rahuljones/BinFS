fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .format(true)
        .compile(&["proto/keeper.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/keeper.proto");
    Ok(())
}
