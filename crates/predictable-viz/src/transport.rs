//! The wire format — `05-viz.md` §1.2.
//!
//! **JSON for metadata, Apache Arrow IPC for numbers.** A 5,000-modelpoint ×
//! 480-period `f64` series is 19 MB as JSON and 2.4 MB as an Arrow stream that
//! goes zero-copy into a typed array in the browser; it is also the format the
//! notebook path already has (`results.to_arrow()`), so there is one wire format
//! and not two.

use arrow_array::RecordBatch;
use arrow_ipc::writer::StreamWriter;

/// The content type of an Arrow IPC stream response.
pub const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

/// Encode one record batch as an Arrow IPC **stream** (not a file).
///
/// The encoding is a pure function of the batch: identical batches produce
/// identical bytes, which is what makes the conformance suite's determinism case
/// meaningful and what lets a governance pack embed the bytes reproducibly.
pub fn to_ipc(batch: &RecordBatch) -> Result<Vec<u8>, arrow_schema::ArrowError> {
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut buffer, &batch.schema())?;
        writer.write(batch)?;
        writer.finish()?;
    }
    Ok(buffer)
}

/// Decode an Arrow IPC stream back into batches — the mirror of [`to_ipc`],
/// used by the tests and by any Rust client of the server's API.
pub fn from_ipc(bytes: &[u8]) -> Result<Vec<RecordBatch>, arrow_schema::ArrowError> {
    let reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None)?;
    reader.collect()
}
