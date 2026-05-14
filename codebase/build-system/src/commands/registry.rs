// gradient registry-serve — minimal HTTP package registry backend.
//
// Launch-tier #369 service. It exposes the same on-disk layout used by
// file:// publish/install, plus HTTP PUT upload with sigstore-identity auth.

use crate::name_validation::safe_cache_path;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process;

const IDENTITY_HEADER: &str = "x-gradient-sigstore-identity";

#[derive(Debug, Clone)]
pub struct RegistryServeOptions<'a> {
    pub root: &'a str,
    pub addr: &'a str,
    pub auth_identity: Option<&'a str>,
    pub max_requests: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpResponse {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct IndexResponse {
    schema_version: u32,
    package: String,
    versions: Vec<String>,
}

pub fn execute(options: RegistryServeOptions<'_>) {
    if let Err(e) = serve(options) {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

pub fn serve(options: RegistryServeOptions<'_>) -> Result<(), String> {
    let root = PathBuf::from(options.root);
    fs::create_dir_all(&root)
        .map_err(|e| format!("Failed to create registry root `{}`: {e}", root.display()))?;
    let listener = TcpListener::bind(options.addr)
        .map_err(|e| format!("Failed to bind registry backend at {}: {e}", options.addr))?;
    println!(
        "Gradient registry listening on {}",
        listener_addr(&listener)
    );
    println!("  root: {}", root.display());
    if let Some(identity) = options.auth_identity {
        println!("  upload auth: sigstore identity `{identity}`");
    } else {
        println!("  upload auth: disabled");
    }

    for (handled, stream) in listener.incoming().enumerate() {
        let stream = stream.map_err(|e| format!("Failed to accept registry request: {e}"))?;
        handle_stream(stream, &root, options.auth_identity)?;
        if options.max_requests.is_some_and(|max| handled + 1 >= max) {
            break;
        }
    }
    Ok(())
}

fn listener_addr(listener: &TcpListener) -> String {
    listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

fn handle_stream(
    mut stream: TcpStream,
    root: &Path,
    auth_identity: Option<&str>,
) -> Result<(), String> {
    let request = read_request(&mut stream)?;
    let response = handle_request(root, auth_identity, &request);
    write_response(&mut stream, &response)
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut header_end = None;
    while header_end.is_none() {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("Failed to read HTTP request: {e}"))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        header_end = find_header_end(&buf);
        if buf.len() > 64 * 1024 {
            return Err("HTTP request headers exceed 64KiB".to_string());
        }
    }
    let header_end = header_end
        .ok_or_else(|| "Malformed HTTP request: missing header terminator".to_string())?;
    let header_text = std::str::from_utf8(&buf[..header_end])
        .map_err(|e| format!("Malformed HTTP request headers: {e}"))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "Malformed HTTP request: missing request line".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "Malformed HTTP request: missing method".to_string())?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| "Malformed HTTP request: missing path".to_string())?
        .to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .map(|v| v.parse::<usize>())
        .transpose()
        .map_err(|e| format!("Invalid Content-Length header: {e}"))?
        .unwrap_or(0);
    let body_start = header_end + 4;
    let mut body = buf.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < content_length {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("Failed to read HTTP request body: {e}"))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn write_response(stream: &mut TcpStream, response: &HttpResponse) -> Result<(), String> {
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.reason,
        response.content_type,
        response.body.len()
    )
    .map_err(|e| format!("Failed to write HTTP response headers: {e}"))?;
    stream
        .write_all(&response.body)
        .map_err(|e| format!("Failed to write HTTP response body: {e}"))
}

fn handle_request(root: &Path, auth_identity: Option<&str>, request: &HttpRequest) -> HttpResponse {
    if request.method == "GET" && request.path == "/healthz" {
        return text(200, "OK", "ok\n");
    }

    let segments: Vec<&str> = request.path.trim_start_matches('/').split('/').collect();
    match (request.method.as_str(), segments.as_slice()) {
        ("GET", ["v1", "packages", package, "index.json"]) => index(root, package),
        ("GET", ["v1", "packages", package, version, filename]) => {
            read_package_file(root, package, version, filename)
        }
        ("PUT", ["v1", "packages", package, version, filename]) => {
            if let Err(response) = check_upload_auth(auth_identity, request) {
                return response;
            }
            write_package_file(root, package, version, filename, &request.body)
        }
        _ => text(404, "Not Found", "not found\n"),
    }
}

fn check_upload_auth(
    auth_identity: Option<&str>,
    request: &HttpRequest,
) -> Result<(), HttpResponse> {
    let Some(expected) = auth_identity else {
        return Ok(());
    };
    match request.headers.get(IDENTITY_HEADER) {
        Some(actual) if actual == expected => Ok(()),
        Some(_) => Err(text(403, "Forbidden", "sigstore identity mismatch\n")),
        None => Err(text(401, "Unauthorized", "missing sigstore identity\n")),
    }
}

fn index(root: &Path, package: &str) -> HttpResponse {
    let package_dir = match safe_cache_path(root, package, "0.0.0") {
        Ok(path) => path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.join(package)),
        Err(e) => return text(400, "Bad Request", &format!("invalid package: {e}\n")),
    };
    let mut versions = Vec::new();
    match fs::read_dir(&package_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let version = entry.file_name().to_string_lossy().to_string();
                if safe_cache_path(root, package, &version).is_ok() {
                    versions.push(version);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return text(
                500,
                "Internal Server Error",
                &format!("failed to read index: {e}\n"),
            )
        }
    }
    versions.sort();
    json(&IndexResponse {
        schema_version: 1,
        package: package.to_string(),
        versions,
    })
}

fn read_package_file(root: &Path, package: &str, version: &str, filename: &str) -> HttpResponse {
    let path = match package_file_path(root, package, version, filename) {
        Ok(path) => path,
        Err(e) => return text(400, "Bad Request", &format!("{e}\n")),
    };
    match fs::read(&path) {
        Ok(bytes) => HttpResponse {
            status: 200,
            reason: "OK",
            content_type: content_type(filename),
            body: bytes,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => text(404, "Not Found", "not found\n"),
        Err(e) => text(
            500,
            "Internal Server Error",
            &format!("failed to read package file: {e}\n"),
        ),
    }
}

fn write_package_file(
    root: &Path,
    package: &str,
    version: &str,
    filename: &str,
    body: &[u8],
) -> HttpResponse {
    let path = match package_file_path(root, package, version, filename) {
        Ok(path) => path,
        Err(e) => return text(400, "Bad Request", &format!("{e}\n")),
    };
    let Some(parent) = path.parent() else {
        return text(400, "Bad Request", "package file has no parent directory\n");
    };
    if let Err(e) = fs::create_dir_all(parent) {
        return text(
            500,
            "Internal Server Error",
            &format!("failed to create package dir: {e}\n"),
        );
    }
    if let Err(e) = fs::write(&path, body) {
        return text(
            500,
            "Internal Server Error",
            &format!("failed to write package file: {e}\n"),
        );
    }
    text(201, "Created", "stored\n")
}

fn package_file_path(
    root: &Path,
    package: &str,
    version: &str,
    filename: &str,
) -> Result<PathBuf, String> {
    let dir = safe_cache_path(root, package, version)
        .map_err(|e| format!("invalid package or version: {e}"))?;
    if !is_allowed_package_file(package, version, filename) {
        return Err("unsupported package filename".to_string());
    }
    Ok(dir.join(filename))
}

fn is_allowed_package_file(package: &str, version: &str, filename: &str) -> bool {
    filename == "gradient-package.toml"
        || filename == format!("{package}-{version}.gradient-pkg")
        || filename == format!("{package}-{version}.publish.json")
        || filename == format!("{package}-{version}.sigstore.json")
}

fn content_type(filename: &str) -> &'static str {
    if filename.ends_with(".json") {
        "application/json"
    } else if filename.ends_with(".toml") {
        "application/toml"
    } else {
        "application/octet-stream"
    }
}

fn json<T: Serialize>(value: &T) -> HttpResponse {
    match serde_json::to_vec_pretty(value) {
        Ok(body) => HttpResponse {
            status: 200,
            reason: "OK",
            content_type: "application/json",
            body,
        },
        Err(e) => text(
            500,
            "Internal Server Error",
            &format!("failed to serialize json: {e}\n"),
        ),
    }
}

fn text(status: u16, reason: &'static str, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        reason,
        content_type: "text/plain; charset=utf-8",
        body: body.as_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, path: &str) -> HttpRequest {
        HttpRequest {
            method: method.to_string(),
            path: path.to_string(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn index_endpoint_lists_versions() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("demo_pkg/1.0.0")).unwrap();
        fs::create_dir_all(tmp.path().join("demo_pkg/1.2.0")).unwrap();

        let response = handle_request(
            tmp.path(),
            None,
            &request("GET", "/v1/packages/demo_pkg/index.json"),
        );

        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).unwrap();
        assert!(body.contains("\"package\": \"demo_pkg\""));
        assert!(body.contains("\"1.0.0\""));
        assert!(body.contains("\"1.2.0\""));
    }

    #[test]
    fn put_requires_matching_sigstore_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let mut request = request(
            "PUT",
            "/v1/packages/demo_pkg/1.0.0/demo_pkg-1.0.0.publish.json",
        );
        request.body = br#"{"schema_version":1}"#.to_vec();

        let missing = handle_request(tmp.path(), Some("sig-123"), &request);
        assert_eq!(missing.status, 401);

        request
            .headers
            .insert(IDENTITY_HEADER.to_string(), "wrong".to_string());
        let wrong = handle_request(tmp.path(), Some("sig-123"), &request);
        assert_eq!(wrong.status, 403);

        request
            .headers
            .insert(IDENTITY_HEADER.to_string(), "sig-123".to_string());
        let stored = handle_request(tmp.path(), Some("sig-123"), &request);
        assert_eq!(stored.status, 201);
        assert!(tmp
            .path()
            .join("demo_pkg/1.0.0/demo_pkg-1.0.0.publish.json")
            .is_file());
    }

    #[test]
    fn get_rejects_unsupported_filenames() {
        let tmp = tempfile::tempdir().unwrap();
        let response = handle_request(
            tmp.path(),
            None,
            &request("GET", "/v1/packages/demo_pkg/1.0.0/../../pwnd"),
        );
        assert_eq!(response.status, 404);

        let response = handle_request(
            tmp.path(),
            None,
            &request("GET", "/v1/packages/demo_pkg/1.0.0/pwnd.txt"),
        );
        assert_eq!(response.status, 400);
    }
}
