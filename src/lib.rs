#![no_std]

//! Bounded HTTP/1.1 protocol core for `webserv`.
//!
//! This crate deliberately owns no sockets, filesystem handles, threads, or
//! global process state. An EuryOS adapter supplies those capabilities around
//! this parser and response encoder.

extern crate alloc;

use alloc::vec::Vec;

/// Maximum request header block accepted by the first server slice.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// Maximum request-target length accepted by the first server slice.
pub const MAX_TARGET_BYTES: usize = 2048;
/// Maximum number of request headers retained by the first server slice.
pub const MAX_HEADERS: usize = 32;

/// A request parsing result. Incomplete input is distinct from malformed input
/// so a socket adapter can wait for more bytes without treating a slow client
/// as hostile.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseResult<'a> {
    Complete(Request<'a>),
    Incomplete,
    Rejected(ParseError),
}

/// HTTP request parsing failure.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    TooLarge,
    InvalidRequestLine,
    UnsupportedMethod,
    InvalidTarget,
    InvalidVersion,
    TooManyHeaders,
    InvalidHeader,
    MissingHost,
}

/// Methods supported by the first static-file server slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
}

/// An HTTP version accepted by the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    Http10,
    Http11,
}

/// Borrowed HTTP request. Header names are compared case-insensitively by
/// [`Request::header`].
#[derive(Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub method: Method,
    /// The path component only; query text is intentionally retained in
    /// `query` rather than being passed to filesystem lookup.
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub version: Version,
    headers: Vec<Header<'a>>,
}

impl<'a> Request<'a> {
    /// Find a header by ASCII case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&'a str> {
        self.headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value)
    }

    /// Iterate over the bounded set of parsed headers.
    pub fn headers(&self) -> impl Iterator<Item = (&'a str, &'a str)> + '_ {
        self.headers
            .iter()
            .map(|header| (header.name, header.value))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Header<'a> {
    name: &'a str,
    value: &'a str,
}

/// Parse one complete HTTP request header block.
pub fn parse_request(input: &[u8]) -> ParseResult<'_> {
    if input.len() > MAX_REQUEST_BYTES {
        return ParseResult::Rejected(ParseError::TooLarge);
    }
    let Some(end) = find_header_end(input) else {
        return ParseResult::Incomplete;
    };
    let head = &input[..end];
    let mut lines = head.split(|byte| *byte == b'\n');
    let Some(request_line) = lines.next() else {
        return ParseResult::Rejected(ParseError::InvalidRequestLine);
    };
    let request_line = trim_cr(request_line);
    let mut fields = request_line.split(|byte| *byte == b' ');
    let (Some(method), Some(target), Some(version), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return ParseResult::Rejected(ParseError::InvalidRequestLine);
    };
    let method = match method {
        b"GET" => Method::Get,
        b"HEAD" => Method::Head,
        _ => return ParseResult::Rejected(ParseError::UnsupportedMethod),
    };
    let version = match version {
        b"HTTP/1.0" => Version::Http10,
        b"HTTP/1.1" => Version::Http11,
        _ => return ParseResult::Rejected(ParseError::InvalidVersion),
    };
    if target.is_empty() || target.len() > MAX_TARGET_BYTES || target[0] != b'/' {
        return ParseResult::Rejected(ParseError::InvalidTarget);
    }
    let target = match core::str::from_utf8(target) {
        Ok(target) => target,
        Err(_) => return ParseResult::Rejected(ParseError::InvalidTarget),
    };
    let (path, query) = target
        .split_once('?')
        .map_or((target, None), |(path, query)| (path, Some(query)));
    if path.is_empty() || !safe_path(path) {
        return ParseResult::Rejected(ParseError::InvalidTarget);
    }

    let mut headers = Vec::new();
    for raw_line in lines {
        let raw_line = trim_cr(raw_line);
        if raw_line.is_empty() {
            continue;
        }
        let Some(colon) = raw_line.iter().position(|byte| *byte == b':') else {
            return ParseResult::Rejected(ParseError::InvalidHeader);
        };
        if headers.len() == MAX_HEADERS {
            return ParseResult::Rejected(ParseError::TooManyHeaders);
        }
        let name = match core::str::from_utf8(&raw_line[..colon]) {
            Ok(name) if valid_header_name(name) => name,
            _ => return ParseResult::Rejected(ParseError::InvalidHeader),
        };
        let value = match core::str::from_utf8(&raw_line[colon + 1..]) {
            Ok(value) => value.trim(),
            Err(_) => return ParseResult::Rejected(ParseError::InvalidHeader),
        };
        headers.push(Header { name, value });
    }
    if version == Version::Http11
        && !headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("host"))
    {
        return ParseResult::Rejected(ParseError::MissingHost);
    }

    ParseResult::Complete(Request {
        method,
        path,
        query,
        version,
        headers,
    })
}

/// HTTP response status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    NotFound,
    BadRequest,
    MethodNotAllowed,
    RequestHeaderFieldsTooLarge,
    InternalServerError,
}

impl Status {
    pub const fn code(self) -> u16 {
        match self {
            Self::Ok => 200,
            Self::NotFound => 404,
            Self::BadRequest => 400,
            Self::MethodNotAllowed => 405,
            Self::RequestHeaderFieldsTooLarge => 431,
            Self::InternalServerError => 500,
        }
    }

    const fn reason(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::NotFound => "Not Found",
            Self::BadRequest => "Bad Request",
            Self::MethodNotAllowed => "Method Not Allowed",
            Self::RequestHeaderFieldsTooLarge => "Request Header Fields Too Large",
            Self::InternalServerError => "Internal Server Error",
        }
    }
}

/// Encode a complete HTTP response. The body is omitted for HEAD responses by
/// the caller; this function always emits the supplied bytes and its length.
pub fn encode_response(status: Status, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = response_headers(status, content_type, body.len());
    response.extend_from_slice(body);
    response
}

/// Encode a response for a HEAD request. The declared content length is the
/// length the corresponding GET response would have, but no body bytes are
/// emitted.
pub fn encode_head_response(status: Status, content_type: &str, content_length: usize) -> Vec<u8> {
    response_headers(status, content_type, content_length)
}

fn response_headers(status: Status, content_type: &str, content_length: usize) -> Vec<u8> {
    let mut response = Vec::with_capacity(128 + content_length.min(MAX_REQUEST_BYTES));
    response.extend_from_slice(b"HTTP/1.1 ");
    push_decimal(&mut response, status.code());
    response.extend_from_slice(b" ");
    response.extend_from_slice(status.reason().as_bytes());
    response.extend_from_slice(b"\r\nContent-Length: ");
    push_decimal(&mut response, content_length as u64);
    response.extend_from_slice(b"\r\nContent-Type: ");
    response.extend_from_slice(content_type.as_bytes());
    response.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    response
}

fn find_header_end(input: &[u8]) -> Option<usize> {
    input
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|end| end + 4)
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn safe_path(path: &str) -> bool {
    !path
        .split('/')
        .any(|component| component == ".." || component.contains('\0'))
}

fn push_decimal(output: &mut Vec<u8>, value: impl Into<u64>) {
    let mut digits = [0u8; 20];
    let mut value = value.into();
    let mut cursor = digits.len();
    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    output.extend_from_slice(&digits[cursor..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::String, vec};

    #[test]
    fn parses_get_with_query_and_case_insensitive_headers() {
        let request =
            b"GET /index.html?theme=dark HTTP/1.1\r\nHost: example\r\nX-Test: yes\r\n\r\n";
        let ParseResult::Complete(request) = parse_request(request) else {
            panic!("request should parse");
        };
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/index.html");
        assert_eq!(request.query, Some("theme=dark"));
        assert_eq!(request.header("host"), Some("example"));
        assert_eq!(request.header("X-TEST"), Some("yes"));
    }

    #[test]
    fn waits_for_the_end_of_headers() {
        assert_eq!(
            parse_request(b"GET / HTTP/1.1\r\nHost: example\r\n"),
            ParseResult::Incomplete
        );
    }

    #[test]
    fn rejects_http11_without_host() {
        assert_eq!(
            parse_request(b"GET / HTTP/1.1\r\n\r\n"),
            ParseResult::Rejected(ParseError::MissingHost)
        );
    }

    #[test]
    fn rejects_path_traversal() {
        assert_eq!(
            parse_request(b"GET /public/../secret HTTP/1.1\r\nHost: example\r\n\r\n"),
            ParseResult::Rejected(ParseError::InvalidTarget)
        );
    }

    #[test]
    fn accepts_head_without_body_semantics_in_parser() {
        let ParseResult::Complete(request) = parse_request(b"HEAD / HTTP/1.0\r\n\r\n") else {
            panic!("request should parse");
        };
        assert_eq!(request.method, Method::Head);
        assert_eq!(request.version, Version::Http10);
    }

    #[test]
    fn bounds_header_count_and_request_size() {
        let mut request = b"GET / HTTP/1.1\r\nHost: example\r\n".to_vec();
        for index in 0..MAX_HEADERS {
            request.extend_from_slice(format_header(index).as_bytes());
        }
        request.extend_from_slice(b"\r\n");
        assert_eq!(
            parse_request(&request),
            ParseResult::Rejected(ParseError::TooManyHeaders)
        );
        assert_eq!(
            parse_request(&vec![b'x'; MAX_REQUEST_BYTES + 1]),
            ParseResult::Rejected(ParseError::TooLarge)
        );
    }

    #[test]
    fn encodes_status_headers_and_body() {
        let response = encode_response(Status::Ok, "text/plain; charset=utf-8", b"hello");
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert!(response
            .windows(19)
            .any(|window| window == b"Content-Length: 5\r\n"));
        assert!(response.ends_with(b"\r\n\r\nhello"));
    }

    #[test]
    fn encodes_head_without_body_and_keeps_get_length() {
        let response = encode_head_response(Status::Ok, "text/plain", 5);
        assert!(response
            .windows(19)
            .any(|window| window == b"Content-Length: 5\r\n"));
        assert!(response.ends_with(b"\r\n\r\n"));
    }

    fn format_header(index: usize) -> String {
        let mut header = String::from("X-Test-");
        push_decimal_string(&mut header, index);
        header.push_str(": value\r\n");
        header
    }

    fn push_decimal_string(output: &mut String, value: usize) {
        if value == 0 {
            output.push('0');
            return;
        }
        let mut digits = [0u8; 20];
        let mut value = value;
        let mut cursor = digits.len();
        while value != 0 {
            cursor -= 1;
            digits[cursor] = b'0' + (value % 10) as u8;
            value /= 10;
        }
        for digit in &digits[cursor..] {
            output.push(*digit as char);
        }
    }
}
