fn main() -> std::io::Result<()> {
    // v3 protocol (RFC-021) — the only supported wire protocol.
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
