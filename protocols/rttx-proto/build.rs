fn main() -> std::io::Result<()> {
    let mut config = prost_build::Config::new();
    config.bytes(["Delta.data", "Input.data", "PaneSnapshot.scrollback"]);
    config.compile_protos(&["proto/rttx.proto"], &["proto/"])?;
    Ok(())
}
