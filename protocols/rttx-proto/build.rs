fn main() -> std::io::Result<()> {
    // v2 protocol (current)
    let mut v2 = prost_build::Config::new();
    v2.bytes(["Delta.data", "Input.data", "PaneSnapshot.scrollback"]);
    v2.compile_protos(&["proto/rttx.proto"], &["proto/"])?;

    // v3 protocol (RFC-021)
    let mut v3 = prost_build::Config::new();
    v3.bytes([
        "rttx.v3.OutputDelta.data",
        "rttx.v3.RawInput.data",
        "rttx.v3.PasteInput.text",
        "rttx.v3.PaneSnapshot.scrollback_tail",
        "rttx.v3.ScrollbackChunk.data",
    ]);
    v3.compile_protos(&["proto/rttx-v3.proto"], &["proto/"])?;

    Ok(())
}
