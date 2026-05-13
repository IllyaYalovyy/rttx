//! Ring buffer flight recorder backed by a persistent file.
//!
//! Stores span events in a fixed-size ring buffer that survives crashes.
//! Uses positioned writes (`pwrite`) to an open file descriptor — the OS
//! page cache keeps hot data in memory while the kernel flushes dirty pages
//! on crash, providing crash persistence without explicit fsync.
//!
//! Layout (4,194,368 bytes total):
//! - Header (64 bytes): magic, version, `write_pos`, padding
//! - Slots (65,536 × 64 bytes): event records

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Magic bytes identifying a valid flight recorder file.
const MAGIC: [u8; 8] = *b"RTTXFLT\0";

/// Current file format version.
const VERSION: u32 = 1;

/// Size of the file header in bytes (cache-line aligned).
const HEADER_SIZE: usize = 64;

/// Size of each event slot in bytes (cache-line aligned).
const SLOT_SIZE: usize = 64;

/// Number of event slots in the ring buffer.
const SLOT_COUNT: usize = 65_536;

/// Total file size: header + slots.
const FILE_SIZE: u64 = (HEADER_SIZE + SLOT_COUNT * SLOT_SIZE) as u64;

/// Offset of the `write_pos` field within the header.
const WRITE_POS_OFFSET: u64 = 12;

/// Event types recorded in the flight buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EventType {
    Enter = 0,
    Exit = 1,
    Event = 2,
    Panic = 3,
}

/// Span kinds identifying the subsystem that produced the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum SpanKind {
    MutexAcquire = 0,
    PtyRead = 1,
    VteParse = 2,
    ClientDispatch = 3,
    ClientWrite = 4,
    ChannelSend = 5,
    SerializationTick = 6,
    IoFlush = 7,
    ClientSession = 8,
    Shutdown = 9,
}

/// A single flight recorder event (64 bytes when serialized).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlightEvent {
    /// Nanoseconds since daemon start.
    pub timestamp_ns: u64,
    /// Sequential span ID (wraps).
    pub span_id: u32,
    /// Type of event.
    pub event_type: EventType,
    /// Subsystem that produced the event.
    pub span_kind: SpanKind,
    /// Context identifier (`pane_id` or `client_id` bytes).
    pub context: [u8; 16],
    /// Associated value (`duration_ns`, `bytes_count`, or `channel_depth`).
    pub value: u64,
}

/// Lock-free ring buffer writer backed by positioned file I/O.
///
/// The writer maintains an in-memory atomic write position and writes
/// each event directly to the file at the correct offset using `pwrite`.
/// The file is pre-allocated and kept open; the OS page cache provides
/// memory-speed writes while the kernel ensures dirty pages reach disk
/// on crash.
#[derive(Debug)]
pub struct RingWriter {
    file: File,
    write_pos: AtomicU64,
    path: PathBuf,
}

impl RingWriter {
    /// Create a new flight recorder file at the given directory.
    ///
    /// If a stale `flight.bin` exists, it is renamed to `flight.prev.bin`.
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;

        let path = dir.join("flight.bin");
        let prev_path = dir.join("flight.prev.bin");

        if path.exists() {
            let _ = fs::rename(&path, &prev_path);
        }

        let file =
            OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path)?;
        file.set_len(FILE_SIZE)?;

        // Write header
        let mut header = [0u8; HEADER_SIZE];
        header[..8].copy_from_slice(&MAGIC);
        header[8..12].copy_from_slice(&VERSION.to_le_bytes());
        // write_pos at offset 12 is already 0
        file.write_all_at(&header, 0)?;

        Ok(Self { file, write_pos: AtomicU64::new(0), path })
    }

    /// Record an event into the ring buffer.
    ///
    /// Lock-free: single atomic increment determines the slot, then a
    /// positioned write places the event without seeking.
    pub fn record(&self, event: &FlightEvent) {
        let pos = self.write_pos.fetch_add(1, Ordering::Relaxed);
        let slot_index = (pos as usize) % SLOT_COUNT;
        let offset = HEADER_SIZE + slot_index * SLOT_SIZE;

        let mut buf = [0u8; SLOT_SIZE];
        serialize_event(event, &mut buf);
        // Positioned write — no locking needed for single writer
        let _ = self.file.write_all_at(&buf, offset as u64);

        // Update write_pos in header
        let new_pos = pos + 1;
        let _ = self.file.write_all_at(&new_pos.to_le_bytes(), WRITE_POS_OFFSET);
    }

    /// Current write position (number of events written, may exceed `SLOT_COUNT`).
    #[must_use]
    pub fn write_pos(&self) -> u64 {
        self.write_pos.load(Ordering::Relaxed)
    }

    /// Flush the file to disk, ensuring all recorded events are durable.
    pub fn flush(&self) -> io::Result<()> {
        self.file.sync_all()
    }

    /// Path to the flight recorder file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Reader for flight recorder files (current or previous instance).
#[derive(Debug)]
pub struct RingReader {
    file: File,
}

impl RingReader {
    /// Open an existing flight recorder file for reading.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        let len = file.metadata()?.len();
        if len != FILE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("flight file size mismatch: expected {FILE_SIZE}, got {len}"),
            ));
        }

        // Validate header
        let mut header = [0u8; HEADER_SIZE];
        file.read_exact_at(&mut header, 0)?;

        if header[..8] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid magic bytes"));
        }
        let version = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        if version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported version: {version}"),
            ));
        }

        Ok(Self { file })
    }

    /// Read the write position from the file header.
    #[must_use]
    pub fn write_pos(&self) -> u64 {
        let mut buf = [0u8; 8];
        self.file.read_exact_at(&mut buf, WRITE_POS_OFFSET).unwrap_or_default();
        u64::from_le_bytes(buf)
    }

    /// Read all events in chronological order, handling wrap-around.
    ///
    /// Returns events from oldest to newest. If fewer than `SLOT_COUNT`
    /// events have been written, returns only the written events.
    #[must_use]
    pub fn read_all(&self) -> Vec<FlightEvent> {
        let write_pos = self.write_pos();
        if write_pos == 0 {
            return Vec::new();
        }

        let count = write_pos.min(SLOT_COUNT as u64) as usize;
        let mut events = Vec::with_capacity(count);
        let mut buf = [0u8; SLOT_SIZE];

        if write_pos <= SLOT_COUNT as u64 {
            for i in 0..count {
                let offset = (HEADER_SIZE + i * SLOT_SIZE) as u64;
                if self.file.read_exact_at(&mut buf, offset).is_ok() {
                    events.push(deserialize_event(&buf));
                }
            }
        } else {
            // Wrapped: oldest slot is at write_pos % SLOT_COUNT
            let start = (write_pos as usize) % SLOT_COUNT;
            for i in 0..SLOT_COUNT {
                let slot_index = (start + i) % SLOT_COUNT;
                let offset = (HEADER_SIZE + slot_index * SLOT_SIZE) as u64;
                if self.file.read_exact_at(&mut buf, offset).is_ok() {
                    events.push(deserialize_event(&buf));
                }
            }
        }

        events
    }
}

/// Serialize a `FlightEvent` into a 64-byte buffer.
fn serialize_event(event: &FlightEvent, buf: &mut [u8; SLOT_SIZE]) {
    buf[0..8].copy_from_slice(&event.timestamp_ns.to_le_bytes());
    buf[8..12].copy_from_slice(&event.span_id.to_le_bytes());
    buf[12..14].copy_from_slice(&(event.event_type as u16).to_le_bytes());
    buf[14..16].copy_from_slice(&(event.span_kind as u16).to_le_bytes());
    buf[16..32].copy_from_slice(&event.context);
    buf[32..40].copy_from_slice(&event.value.to_le_bytes());
    buf[40..64].fill(0); // reserved padding
}

/// Deserialize a `FlightEvent` from a 64-byte buffer.
fn deserialize_event(buf: &[u8; SLOT_SIZE]) -> FlightEvent {
    let timestamp_ns =
        u64::from_le_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]]);
    let span_id = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let event_type_raw = u16::from_le_bytes([buf[12], buf[13]]);
    let span_kind_raw = u16::from_le_bytes([buf[14], buf[15]]);
    let mut context = [0u8; 16];
    context.copy_from_slice(&buf[16..32]);
    let value = u64::from_le_bytes([
        buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39],
    ]);

    FlightEvent {
        timestamp_ns,
        span_id,
        event_type: match event_type_raw {
            0 => EventType::Enter,
            1 => EventType::Exit,
            3 => EventType::Panic,
            _ => EventType::Event,
        },
        span_kind: match span_kind_raw {
            0 => SpanKind::MutexAcquire,
            1 => SpanKind::PtyRead,
            2 => SpanKind::VteParse,
            3 => SpanKind::ClientDispatch,
            4 => SpanKind::ClientWrite,
            5 => SpanKind::ChannelSend,
            6 => SpanKind::SerializationTick,
            7 => SpanKind::IoFlush,
            8 => SpanKind::ClientSession,
            _ => SpanKind::Shutdown,
        },
        context,
        value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_event(ts: u64, span_id: u32) -> FlightEvent {
        FlightEvent {
            timestamp_ns: ts,
            span_id,
            event_type: EventType::Enter,
            span_kind: SpanKind::PtyRead,
            context: [0; 16],
            value: 42,
        }
    }

    #[test]
    fn file_size_is_exactly_4mb_plus_header() {
        assert_eq!(FILE_SIZE, 4_194_368);
        assert_eq!(SLOT_COUNT * SLOT_SIZE, 4_194_304);
    }

    #[test]
    fn slot_size_is_cache_line_aligned() {
        assert_eq!(SLOT_SIZE, 64);
        assert_eq!(HEADER_SIZE, 64);
    }

    #[test]
    fn write_read_round_trip() {
        let dir = TempDir::new().unwrap();
        let event = sample_event(1_000_000, 1);

        {
            let writer = RingWriter::open(dir.path()).unwrap();
            writer.record(&event);
            assert_eq!(writer.write_pos(), 1);
        }

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], event);
    }

    #[test]
    fn write_multiple_events_preserves_order() {
        let dir = TempDir::new().unwrap();
        let writer = RingWriter::open(dir.path()).unwrap();

        for i in 0..100 {
            writer.record(&sample_event(i * 1000, i as u32));
        }
        assert_eq!(writer.write_pos(), 100);

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), 100);
        for (i, ev) in events.iter().enumerate() {
            assert_eq!(ev.timestamp_ns, (i as u64) * 1000);
            assert_eq!(ev.span_id, i as u32);
        }
    }

    #[test]
    fn wrap_around_returns_chronological_order() {
        let dir = TempDir::new().unwrap();
        let writer = RingWriter::open(dir.path()).unwrap();

        let total = SLOT_COUNT as u64 + 100;
        for i in 0..total {
            writer.record(&sample_event(i, i as u32));
        }
        assert_eq!(writer.write_pos(), total);

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), SLOT_COUNT);

        // Oldest event should be at position 100 (first 100 were overwritten)
        assert_eq!(events[0].timestamp_ns, 100);
        assert_eq!(events[0].span_id, 100);

        // Newest event should be the last written
        assert_eq!(events[SLOT_COUNT - 1].timestamp_ns, total - 1);
        assert_eq!(events[SLOT_COUNT - 1].span_id, (total - 1) as u32);

        // Verify monotonic order
        for i in 1..events.len() {
            assert!(events[i].timestamp_ns > events[i - 1].timestamp_ns);
        }
    }

    #[test]
    fn stale_file_preserved_as_prev() {
        let dir = TempDir::new().unwrap();

        // Create first instance
        {
            let writer = RingWriter::open(dir.path()).unwrap();
            writer.record(&sample_event(111, 1));
        }
        assert!(dir.path().join("flight.bin").exists());

        // Create second instance — should rename old to .prev
        {
            let writer = RingWriter::open(dir.path()).unwrap();
            writer.record(&sample_event(222, 2));
        }

        assert!(dir.path().join("flight.bin").exists());
        assert!(dir.path().join("flight.prev.bin").exists());

        // Prev file should contain the first instance's data
        let prev_reader = RingReader::open(&dir.path().join("flight.prev.bin")).unwrap();
        let prev_events = prev_reader.read_all();
        assert_eq!(prev_events.len(), 1);
        assert_eq!(prev_events[0].timestamp_ns, 111);

        // Current file should contain the second instance's data
        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].timestamp_ns, 222);
    }

    #[test]
    fn crash_persistence_via_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("flight.bin");

        // Write events and drop writer (simulates crash)
        {
            let writer = RingWriter::open(dir.path()).unwrap();
            for i in 0..10 {
                writer.record(&sample_event(i * 100, i as u32));
            }
        }

        // Read back from the file
        let reader = RingReader::open(&path).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), 10);
        assert_eq!(events[0].timestamp_ns, 0);
        assert_eq!(events[9].timestamp_ns, 900);
    }

    #[test]
    fn all_event_types_round_trip() {
        let dir = TempDir::new().unwrap();
        let writer = RingWriter::open(dir.path()).unwrap();

        let events = [
            FlightEvent {
                timestamp_ns: 1,
                span_id: 1,
                event_type: EventType::Enter,
                span_kind: SpanKind::MutexAcquire,
                context: [1; 16],
                value: 100,
            },
            FlightEvent {
                timestamp_ns: 2,
                span_id: 2,
                event_type: EventType::Exit,
                span_kind: SpanKind::ClientWrite,
                context: [2; 16],
                value: 200,
            },
            FlightEvent {
                timestamp_ns: 3,
                span_id: 3,
                event_type: EventType::Event,
                span_kind: SpanKind::Shutdown,
                context: [0xFF; 16],
                value: u64::MAX,
            },
        ];

        for ev in &events {
            writer.record(ev);
        }

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let read_events = reader.read_all();
        assert_eq!(read_events.len(), 3);
        assert_eq!(read_events[0], events[0]);
        assert_eq!(read_events[1], events[1]);
        assert_eq!(read_events[2], events[2]);
    }

    #[test]
    fn all_span_kinds_round_trip() {
        let dir = TempDir::new().unwrap();
        let writer = RingWriter::open(dir.path()).unwrap();

        let kinds = [
            SpanKind::MutexAcquire,
            SpanKind::PtyRead,
            SpanKind::VteParse,
            SpanKind::ClientDispatch,
            SpanKind::ClientWrite,
            SpanKind::ChannelSend,
            SpanKind::SerializationTick,
            SpanKind::IoFlush,
            SpanKind::ClientSession,
            SpanKind::Shutdown,
        ];

        for (i, &kind) in kinds.iter().enumerate() {
            writer.record(&FlightEvent {
                timestamp_ns: i as u64,
                span_id: i as u32,
                event_type: EventType::Event,
                span_kind: kind,
                context: [0; 16],
                value: 0,
            });
        }

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), kinds.len());
        for (i, ev) in events.iter().enumerate() {
            assert_eq!(ev.span_kind, kinds[i]);
        }
    }

    #[test]
    fn reader_rejects_wrong_size() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.bin");
        fs::write(&path, b"too short").unwrap();

        let result = RingReader::open(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("size mismatch"));
    }

    #[test]
    fn reader_rejects_invalid_magic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.bin");

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.set_len(FILE_SIZE).unwrap();

        let result = RingReader::open(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid magic"));
    }

    #[test]
    fn empty_ring_returns_no_events() {
        let dir = TempDir::new().unwrap();
        let _writer = RingWriter::open(dir.path()).unwrap();

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert!(events.is_empty());
    }

    #[test]
    fn context_field_preserves_arbitrary_bytes() {
        let dir = TempDir::new().unwrap();
        let writer = RingWriter::open(dir.path()).unwrap();

        let ctx: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        writer.record(&FlightEvent {
            timestamp_ns: 42,
            span_id: 7,
            event_type: EventType::Enter,
            span_kind: SpanKind::ClientSession,
            context: ctx,
            value: 999,
        });

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events[0].context, ctx);
    }

    #[test]
    fn exact_slot_count_fill_no_wrap() {
        let dir = TempDir::new().unwrap();
        let writer = RingWriter::open(dir.path()).unwrap();

        for i in 0..SLOT_COUNT as u64 {
            writer.record(&sample_event(i, i as u32));
        }

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), SLOT_COUNT);
        assert_eq!(events[0].timestamp_ns, 0);
        assert_eq!(events[SLOT_COUNT - 1].timestamp_ns, (SLOT_COUNT - 1) as u64);
    }

    #[test]
    fn wrap_by_one_slot() {
        let dir = TempDir::new().unwrap();
        let writer = RingWriter::open(dir.path()).unwrap();

        let total = SLOT_COUNT as u64 + 1;
        for i in 0..total {
            writer.record(&sample_event(i, i as u32));
        }

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), SLOT_COUNT);
        // First event (ts=0) was overwritten; oldest is ts=1
        assert_eq!(events[0].timestamp_ns, 1);
        assert_eq!(events[SLOT_COUNT - 1].timestamp_ns, total - 1);
    }
}
