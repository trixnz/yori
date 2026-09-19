use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use bitcode::{Decode, Encode};

use crate::{
    comparison::{Comparison, MergePaths},
    invocation::InvocationRequest,
};

const VERSION: u16 = 2;
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
    directory: WirePath,
    comparisons: Vec<WireComparison>,
}

#[derive(Encode, Decode)]
struct WireResponse {
    version: u16,
    error: Option<String>,
}

pub(crate) fn write_request(
    writer: &mut impl Write,
    invocation: &InvocationRequest,
) -> Result<(), String> {
    let mut total_path_bytes = 0;
    let request = WireRequest {
        version: VERSION,
        directory: encode_path(
            &invocation.directory,
            &mut total_path_bytes,
            "invocation directory",
        )?,
        comparisons: encode_comparisons(&invocation.comparisons, &mut total_path_bytes)?,
    };

    write_frame(writer, &bitcode::encode(&request), MAX_REQUEST_FRAME_BYTES)
}

pub(crate) fn read_request(reader: &mut impl Read) -> Result<InvocationRequest, String> {
    let frame = read_frame(reader, MAX_REQUEST_FRAME_BYTES)?;
    let request: WireRequest = bitcode::decode(&frame)
        .map_err(|error| format!("request has invalid encoding: {error}"))?;
    if request.version != VERSION {
        return Err(format!(
            "request uses unsupported protocol version {}; expected {VERSION}",
            request.version
        ));
    }

    let mut total_path_bytes = 0;
    let directory = decode_path(
        &request.directory,
        &mut total_path_bytes,
        "invocation directory",
    )?;
    let comparisons = decode_comparisons(request.comparisons, &mut total_path_bytes)?;

    Ok(InvocationRequest::new(directory, comparisons))
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

fn encode_comparisons(
    comparisons: &[Comparison],
    total_path_bytes: &mut usize,
) -> Result<Vec<WireComparison>, String> {
    if comparisons.len() > MAX_COMPARISONS {
        return Err("too many comparisons in one request".into());
    }

    comparisons
        .iter()
        .map(|comparison| {
            let paths = comparison.wire_paths()?;

            match comparison {
                Comparison::Diff(_) => Ok(WireComparison::Diff {
                    baseline: encode_path(paths[0], total_path_bytes, "file path")?,
                    local: encode_path(paths[1], total_path_bytes, "file path")?,
                }),
                Comparison::Merge(_) => Ok(WireComparison::Merge {
                    base: encode_path(paths[0], total_path_bytes, "file path")?,
                    local: encode_path(paths[1], total_path_bytes, "file path")?,
                    incoming: encode_path(paths[2], total_path_bytes, "file path")?,
                    result: encode_path(paths[3], total_path_bytes, "file path")?,
                }),
            }
        })
        .collect()
}

fn decode_comparisons(
    comparisons: Vec<WireComparison>,
    total_path_bytes: &mut usize,
) -> Result<Vec<Comparison>, String> {
    if comparisons.len() > MAX_COMPARISONS {
        return Err("too many comparisons in one request".into());
    }

    comparisons
        .into_iter()
        .map(|comparison| match comparison {
            WireComparison::Diff { baseline, local } => Ok(Comparison::diff(
                decode_path(&baseline, total_path_bytes, "file path")?,
                decode_path(&local, total_path_bytes, "file path")?,
            )),
            WireComparison::Merge {
                base,
                local,
                incoming,
                result,
            } => Ok(Comparison::Merge(MergePaths {
                base: decode_path(&base, total_path_bytes, "file path")?,
                local: decode_path(&local, total_path_bytes, "file path")?,
                incoming: decode_path(&incoming, total_path_bytes, "file path")?,
                result: decode_path(&result, total_path_bytes, "file path")?,
            })),
        })
        .collect()
}

fn encode_path(path: &Path, total_path_bytes: &mut usize, field: &str) -> Result<WirePath, String> {
    if !path.is_absolute() {
        return Err(format!("{field} must be absolute"));
    }

    let encoded =
        encode_native_path(path.as_os_str()).map_err(|error| format!("{field} {error}"))?;
    add_path_bytes(total_path_bytes, encoded.len())?;
    Ok(WirePath(encoded))
}

fn decode_path(
    path: &WirePath,
    total_path_bytes: &mut usize,
    field: &str,
) -> Result<PathBuf, String> {
    add_path_bytes(total_path_bytes, path.0.len())?;
    let path =
        PathBuf::from(decode_native_path(&path.0).map_err(|error| format!("{field} {error}"))?);
    if !path.is_absolute() {
        return Err(format!("{field} must be absolute"));
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
        return Err("must not contain NUL bytes".into());
    }

    Ok(bytes.to_vec())
}

#[cfg(unix)]
fn decode_native_path(bytes: &[u8]) -> Result<OsString, String> {
    use std::os::unix::ffi::OsStringExt;

    if bytes.contains(&0) {
        return Err("must not contain NUL bytes".into());
    }

    Ok(OsString::from_vec(bytes.to_vec()))
}

#[cfg(windows)]
fn encode_native_path(path: &std::ffi::OsStr) -> Result<Vec<u8>, String> {
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = Vec::new();
    for unit in path.encode_wide() {
        if unit == 0 {
            return Err("must not contain NUL characters".into());
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
        return Err("has an invalid native encoding length".into());
    }

    let units = pairs
        .iter()
        .map(|bytes| u16::from_le_bytes(*bytes))
        .collect::<Vec<_>>();
    if units.contains(&0) {
        return Err("must not contain NUL characters".into());
    }

    Ok(OsString::from_wide(&units))
}

fn io_error(error: std::io::Error) -> String {
    let message = error.to_string();
    drop(error);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_request(version: u16, directory: WirePath) -> Vec<u8> {
        let request = WireRequest {
            version,
            directory,
            comparisons: Vec::new(),
        };
        let mut frame = Vec::new();
        write_frame(
            &mut frame,
            &bitcode::encode(&request),
            MAX_REQUEST_FRAME_BYTES,
        )
        .unwrap();
        frame
    }

    fn wire_path(path: &Path) -> WirePath {
        WirePath(encode_native_path(path.as_os_str()).unwrap())
    }

    #[cfg(unix)]
    fn malformed_absolute_path() -> WirePath {
        WirePath(b"/invalid\0directory".to_vec())
    }

    #[cfg(windows)]
    fn malformed_absolute_path() -> WirePath {
        let mut bytes = encode_native_path(Path::new(r"C:\invalid").as_os_str()).unwrap();
        bytes.push(0);
        WirePath(bytes)
    }

    #[test]
    fn request_round_trip_preserves_the_invocation_directory() {
        let directory = tempfile::tempdir().unwrap();
        let invocation = InvocationRequest::new(directory.path().to_owned(), Vec::new());
        let mut frame = Vec::new();

        write_request(&mut frame, &invocation).unwrap();

        assert_eq!(read_request(&mut frame.as_slice()).unwrap(), invocation);
    }

    #[test]
    fn request_rejects_an_unsupported_version_with_expected_version() {
        let directory = tempfile::tempdir().unwrap();
        let frame = encoded_request(VERSION + 1, wire_path(directory.path()));

        let error = read_request(&mut frame.as_slice()).unwrap_err();

        assert!(error.contains("unsupported protocol version"));
        assert!(error.contains(&VERSION.to_string()));
    }

    #[test]
    fn request_rejects_relative_malformed_and_oversized_directories() {
        let relative = encoded_request(VERSION, wire_path(Path::new("relative")));
        assert!(
            read_request(&mut relative.as_slice())
                .unwrap_err()
                .contains("invocation directory must be absolute")
        );

        let malformed = encoded_request(VERSION, malformed_absolute_path());
        assert!(
            read_request(&mut malformed.as_slice())
                .unwrap_err()
                .contains("invocation directory")
        );

        let oversized = encoded_request(VERSION, WirePath(vec![b'/'; MAX_PATH_BYTES + 1]));
        assert!(
            read_request(&mut oversized.as_slice())
                .unwrap_err()
                .contains("path data exceeds request size limit")
        );
    }
}
