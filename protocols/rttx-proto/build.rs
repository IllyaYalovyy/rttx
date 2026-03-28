fn main() -> std::io::Result<()> {
    prost_build::compile_protos(&["proto/rttx.proto"], &["proto/"])?;
    Ok(())
}
