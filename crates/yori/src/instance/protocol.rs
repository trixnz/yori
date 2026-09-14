use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use bitcode::{Decode, Encode};

use crate::comparison::{ComparisonPaths, MergePaths};

const VERSION: u16 = 1;
const MAX_COMPARISONS: usize = 128;
const MAX_PATH_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_FRAME_BYTES: usize = MAX_PATH_BYTES + 64 * 1024;
const MAX_RESPONSE_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_FRAME_BYTES: usize = MAX_RESPONSE_MESSAGE_BYTES + 1024;

#[derive(Encode, Decode)]
struct WirePath(Vec<u8>);

#[derive(Encode, Decode)]
enum WireComparison {
    Diff {
        baseline: WirePath,
        local: WirePath,
    },
    Merge {
        base: WirePath,
        local: WirePath,
        incoming: WirePath,
        result: WirePath,
    },
}

#[derive(Encode, Decode)]
struct WireRequest {
    version: u16,
    comparisons: Vec<WireComparison>,
}

#[derive(Encode, Decode)]
struct WireResponse {
    version: u16,
    error: Option<String>,
}

pub(crate) fn write_request(
    writer: &mut impl Write,
    comparisons: &[ComparisonPaths],
) -> Result<(), String> {
    let request = WireRequest {
        version: VERSION,
        comparisons: encode_comparisons(comparisons)?,
    };

    write_frame(writer, &bitcode::encode(&request), MAX_REQUEST_FRAME_BYTES)
}

pub(crate) fn read_request(reader: &mut impl Read) -> Result<Vec<ComparisonPaths>, String> {
    let frame = read_frame(reader, MAX_REQUEST_FRAME_BYTES)?;
    let request: WireRequest = bitcode::decode(&frame)
        .map_err(|error| format!("request has invalid encoding: {error}"))?;
    if request.version != VERSION {
        return Err("request uses an unsupported protocol version".into());
    }

    decode_comparisons(request.comparisons)
}

pub(crate) fn write_response(
    writer: &mut impl Write,
    result: Result<(), String>,
) -> Result<(), String> {
    let error = result.err();
    if error
        .as_ref()
        .is_some_and(|error| error.len() > MAX_RESPONSE_MESSAGE_BYTES)
    {
        return Err("response exceeds size limit".into());
    }

    let response = WireResponse {
        version: VERSION,
        error,
    };
    write_frame(
        writer,
        &bitcode::encode(&response),
        MAX_RESPONSE_FRAME_BYTES,
    )
}

pub(crate) fn read_response(reader: &mut impl Read) -> Result<(), String> {
    let frame = read_frame(reader, MAX_RESPONSE_FRAME_BYTES)?;
    let response: WireResponse = bitcode::decode(&frame)
        .map_err(|error| format!("response has invalid encoding: {error}"))?;
    if response.version != VERSION {
        return Err("response uses an unsupported protocol version".into());
    }

    response.error.map_or(Ok(()), Err)
}

fn write_frame(writer: &mut impl Write, bytes: &[u8], limit: usize) -> Result<(), String> {
    if bytes.len() > limit {
        return Err("message exceeds size limit".into());
    }

    let length: u32 = bytes
        .len()
        .try_into()
        .map_err(|_| "message exceeds size limit")?;
    writer.write_all(&length.to_le_bytes()).map_err(io_error)?;
    writer.write_all(bytes).map_err(io_error)?;
    writer.flush().map_err(io_error)
}

fn read_frame(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut length = [0; size_of::<u32>()];
    reader.read_exact(&mut length).map_err(io_error)?;
    let length =
        usize::try_from(u32::from_le_bytes(length)).map_err(|_| "message exceeds size limit")?;
    if length > limit {
        return Err("message exceeds size limit".into());
    }

    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

fn encode_comparisons(comparisons: &[ComparisonPaths]) -> Result<Vec<WireComparison>, String> {
    if comparisons.len() > MAX_COMPARISONS {
        return Err("too many comparisons in one request".into());
    }

    let mut total_path_bytes = 0;
    comparisons
        .iter()
        .map(|comparison| match comparison {
            ComparisonPaths::Diff { baseline, local } => Ok(WireComparison::Diff {
                baseline: encode_path(baseline, &mut total_path_bytes)?,
                local: encode_path(local, &mut total_path_bytes)?,
            }),
            ComparisonPaths::Merge(paths) => Ok(WireComparison::Merge {
                base: encode_path(&paths.base, &mut total_path_bytes)?,
                local: encode_path(&paths.local, &mut total_path_bytes)?,
                incoming: encode_path(&paths.incoming, &mut total_path_bytes)?,
                result: encode_path(&paths.result, &mut total_path_bytes)?,
            }),
        })
        .collect()
}

fn decode_comparisons(comparisons: Vec<WireComparison>) -> Result<Vec<ComparisonPaths>, String> {
    if comparisons.len() > MAX_COMPARISONS {
        return Err("too many comparisons in one request".into());
    }

    let mut total_path_bytes = 0;
    comparisons
        .into_iter()
        .map(|comparison| match comparison {
            WireComparison::Diff { baseline, local } => Ok(ComparisonPaths::Diff {
                baseline: decode_path(&baseline, &mut total_path_bytes)?,
                local: decode_path(&local, &mut total_path_bytes)?,
            }),
            WireComparison::Merge {
                base,
                local,
                incoming,
                result,
            } => Ok(ComparisonPaths::Merge(MergePaths {
                base: decode_path(&base, &mut total_path_bytes)?,
                local: decode_path(&local, &mut total_path_bytes)?,
                incoming: decode_path(&incoming, &mut total_path_bytes)?,
                result: decode_path(&result, &mut total_path_bytes)?,
            })),
        })
        .collect()
}

fn encode_path(path: &Path, total_path_bytes: &mut usize) -> Result<WirePath, String> {
    if !path.is_absolute() {
        return Err("file paths must be absolute".into());
    }

    let encoded = encode_native_path(path.as_os_str())?;
    add_path_bytes(total_path_bytes, encoded.len())?;
    Ok(WirePath(encoded))
}

fn decode_path(path: &WirePath, total_path_bytes: &mut usize) -> Result<PathBuf, String> {
    add_path_bytes(total_path_bytes, path.0.len())?;
    let path = PathBuf::from(decode_native_path(&path.0)?);
    if !path.is_absolute() {
        return Err("file paths must be absolute".into());
    }

    Ok(path)
}

fn add_path_bytes(total: &mut usize, length: usize) -> Result<(), String> {
    *total = total
        .checked_add(length)
        .ok_or("path data exceeds request size limit")?;
    if *total > MAX_PATH_BYTES {
        return Err("path data exceeds request size limit".into());
    }

    Ok(())
}

#[cfg(unix)]
fn encode_native_path(path: &std::ffi::OsStr) -> Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_bytes();
    if bytes.contains(&0) {
        return Err("file paths must not contain NUL bytes".into());
    }

    Ok(bytes.to_vec())
}

#[cfg(unix)]
fn decode_native_path(bytes: &[u8]) -> Result<OsString, String> {
    use std::os::unix::ffi::OsStringExt;

    if bytes.contains(&0) {
        return Err("file paths must not contain NUL bytes".into());
    }

    Ok(OsString::from_vec(bytes.to_vec()))
}

#[cfg(windows)]
fn encode_native_path(path: &std::ffi::OsStr) -> Result<Vec<u8>, String> {
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = Vec::new();
    for unit in path.encode_wide() {
        if unit == 0 {
            return Err("file paths must not contain NUL characters".into());
        }
        encoded.extend_from_slice(&unit.to_le_bytes());
    }

    Ok(encoded)
}

#[cfg(windows)]
fn decode_native_path(bytes: &[u8]) -> Result<OsString, String> {
    use std::os::windows::ffi::OsStringExt;

    let (pairs, remainder) = bytes.as_chunks::<{ size_of::<u16>() }>();
    if !remainder.is_empty() {
        return Err("Windows path data has an invalid length".into());
    }

    let units = pairs
        .iter()
        .map(|bytes| u16::from_le_bytes(*bytes))
        .collect::<Vec<_>>();
    if units.contains(&0) {
        return Err("file paths must not contain NUL characters".into());
    }

    Ok(OsString::from_wide(&units))
}

fn io_error(error: std::io::Error) -> String {
    let message = error.to_string();
    drop(error);
    message
}
