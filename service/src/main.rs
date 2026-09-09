#![no_std]
#![no_main]

extern crate alloc;

use eury_sdk::abi::{ObjectType, Rights};
use eury_sdk::capability::Handle;
use eury_sdk::fs::{FsClient, IpcFsChannel, ShellSession};
use eury_sdk::idl::Serialize;
use eury_sdk::net::{
    IpAddr, SocketAccept, SocketAddr, SocketBind, SocketClose, SocketCreate, SocketId,
    SocketListen, SocketProtocol, SocketRecv, SocketSend, MAX_CLIENT_PAYLOAD_BYTES,
};
use eury_sdk::{info, runtime as sys};
use webserv_core::{
    encode_head_response, encode_response, parse_request, Method, ParseError, ParseResult, Status,
    MAX_REQUEST_BYTES,
};

include!(concat!(env!("OUT_DIR"), "/grants.rs"));

eury_sdk::main!("webserv", webserv_main);

const HTTP_PORT: u16 = 80;
const REQUEST_TIMEOUT_US: u64 = 5_000_000;
const HEAP_BYTES: usize = 2 * 1024 * 1024;
const XFER_BYTES: usize = 88;
const PAYLOAD_OFFSET: u64 = 256;
const NOT_FOUND_BODY: &[u8] = b"not found\n";
const BAD_REQUEST_BODY: &[u8] = b"bad request\n";
const METHOD_BODY: &[u8] = b"method not allowed\n";

fn webserv_main() -> ! {
    eury_sdk::process::init_heap_dynamic(HEAP_BYTES, "webserv");

    let grants = Grants::from_startup().unwrap_or_else(|| {
        info!(
            "webserv",
            "named grant directory missing or invalid; parking"
        );
        eury_sdk::process::park();
    });
    let net_ep = required_grant(
        grants.net_ep(),
        "net_ep",
        ObjectType::Endpoint,
        Rights::READ | Rights::WRITE,
    );
    let net_buf = required_grant(
        grants.net_buf(),
        "net_buf",
        ObjectType::MemoryObject,
        Rights::READ | Rights::WRITE | Rights::MAP,
    );
    let fs_ep = required_grant(
        grants.fs_ep(),
        "fs_ep",
        ObjectType::Endpoint,
        Rights::READ | Rights::WRITE,
    );
    let fs_buf = required_grant(
        grants.fs_buf(),
        "fs_buf",
        ObjectType::MemoryObject,
        Rights::READ | Rights::WRITE | Rights::MAP,
    );
    let (net_buf_va, fs_buf_va) = map_session_pages(net_buf, fs_buf);
    let mut fs_channel = IpcFsChannel::new(fs_ep, fs_buf_va);
    let mut files = ShellSession::new(FsClient::with_channel(&mut fs_channel));

    let listener = loop {
        if let Some(socket) = setup_listener(net_ep, net_buf_va, net_buf) {
            break socket;
        }
        sys::sleep_us(50_000);
    };
    info!("webserv", "listening on TCP :{}", HTTP_PORT);

    loop {
        let (status, raw_socket) = send_request(
            net_ep,
            net_buf_va,
            &SocketAccept {
                socket_id: listener,
                send_buffer: net_buf,
                recv_buffer: net_buf,
            },
        );
        if status != 0 || raw_socket == 0 {
            sys::sleep_us(10_000);
            continue;
        }

        let socket = SocketId(raw_socket);
        serve_connection(net_ep, net_buf_va, socket, &mut files);
        let _ = send_request(net_ep, net_buf_va, &SocketClose { socket_id: socket });
    }
}

fn required_grant(
    grant: Option<eury_sdk::grants::Grant<'_>>,
    name: &str,
    object_type: ObjectType,
    rights: Rights,
) -> Handle {
    match grant {
        Some(grant) if grant.object_type == object_type && grant.rights.contains(rights) => {
            grant.handle
        }
        Some(grant) => {
            info!(
                "webserv",
                "grant {} has wrong type/rights (type={:?}, rights={:#x}); parking",
                name,
                grant.object_type,
                grant.rights.bits()
            );
            eury_sdk::process::park();
        }
        None => {
            info!("webserv", "required grant {} absent; parking", name);
            eury_sdk::process::park();
        }
    }
}

fn map_session_pages(net_buf: Handle, fs_buf: Handle) -> (u64, u64) {
    let mut net = None;
    let mut fs = None;
    let mut tries = 0u32;
    while net.is_none() || fs.is_none() {
        if net.is_none() {
            let mapped = sys::map(net_buf, 0, sys::MAP_READ | sys::MAP_WRITE | sys::MAP_USER);
            if mapped.is_ok() {
                net = Some(mapped.val1);
            }
        }
        if fs.is_none() {
            let mapped = sys::map(fs_buf, 0, sys::MAP_READ | sys::MAP_WRITE | sys::MAP_USER);
            if mapped.is_ok() {
                fs = Some(mapped.val1);
            }
        }
        if net.is_none() || fs.is_none() {
            tries += 1;
            if tries > 200 {
                info!("webserv", "required session pages never arrived; parking");
                eury_sdk::process::park();
            }
            sys::sleep_us(50_000);
        }
    }
    (net.unwrap(), fs.unwrap())
}

fn setup_listener(ep: Handle, buf_va: u64, buf: Handle) -> Option<SocketId> {
    let (status, raw) = send_request(
        ep,
        buf_va,
        &SocketCreate {
            protocol: SocketProtocol::Tcp,
            send_buffer: buf,
            recv_buffer: buf,
        },
    );
    if status != 0 || raw == 0 {
        return None;
    }
    let socket = SocketId(raw);
    if send_request(
        ep,
        buf_va,
        &SocketBind {
            socket_id: socket,
            addr: SocketAddr {
                ip: IpAddr::V4([0; 4]),
                port: HTTP_PORT,
            },
        },
    )
    .0 != 0
    {
        close_socket(ep, buf_va, socket);
        return None;
    }
    if send_request(
        ep,
        buf_va,
        &SocketListen {
            socket_id: socket,
            backlog: 4,
        },
    )
    .0 != 0
    {
        close_socket(ep, buf_va, socket);
        return None;
    }
    Some(socket)
}

fn serve_connection(ep: Handle, buf_va: u64, socket: SocketId, files: &mut ShellSession<'_>) {
    let mut request_bytes = [0u8; MAX_REQUEST_BYTES];
    let request_len = match receive_request(ep, buf_va, socket, &mut request_bytes) {
        Some(len) => len,
        None => {
            send_body(
                ep,
                buf_va,
                socket,
                Status::BadRequest,
                "text/plain",
                BAD_REQUEST_BODY,
            );
            return;
        }
    };

    let request = match parse_request(&request_bytes[..request_len]) {
        ParseResult::Complete(request) => request,
        ParseResult::Incomplete => {
            send_body(
                ep,
                buf_va,
                socket,
                Status::BadRequest,
                "text/plain",
                BAD_REQUEST_BODY,
            );
            return;
        }
        ParseResult::Rejected(error) => {
            let (status, body) = error_response(error);
            send_body(ep, buf_va, socket, status, "text/plain", body);
            return;
        }
    };

    let path = if request.path == "/" {
        "/index.html"
    } else {
        request.path
    };
    let body = match files.read_file(path) {
        Ok(body) => body,
        Err(_) => {
            send_body(
                ep,
                buf_va,
                socket,
                Status::NotFound,
                "text/plain",
                NOT_FOUND_BODY,
            );
            return;
        }
    };
    let content_type = content_type(path);
    let response = match request.method {
        Method::Get => encode_response(Status::Ok, content_type, &body),
        Method::Head => encode_head_response(Status::Ok, content_type, body.len()),
    };
    if !send_bytes(ep, buf_va, socket, &response) {
        info!("webserv", "response send failed");
    }
}

fn receive_request(
    ep: Handle,
    buf_va: u64,
    socket: SocketId,
    output: &mut [u8; MAX_REQUEST_BYTES],
) -> Option<usize> {
    let deadline = sys::uptime_us().saturating_add(REQUEST_TIMEOUT_US);
    let mut len = 0usize;
    loop {
        let (status, packed) = send_request(ep, buf_va, &SocketRecv { socket_id: socket });
        if status != 0 {
            return None;
        }
        let count = (packed as u32 as usize).min(MAX_CLIENT_PAYLOAD_BYTES);
        if count != 0 {
            if len.checked_add(count)? > output.len() {
                return None;
            }
            unsafe {
                let source =
                    core::slice::from_raw_parts((buf_va + PAYLOAD_OFFSET) as *const u8, count);
                output[len..len + count].copy_from_slice(source);
            }
            len += count;
            if matches!(parse_request(&output[..len]), ParseResult::Complete(_)) {
                return Some(len);
            }
        } else {
            sys::sleep_us(10_000);
        }
        if sys::uptime_us() >= deadline {
            return None;
        }
    }
}

fn send_body(
    ep: Handle,
    buf_va: u64,
    socket: SocketId,
    status: Status,
    content_type: &str,
    body: &[u8],
) {
    let response = encode_response(status, content_type, body);
    let _ = send_bytes(ep, buf_va, socket, &response);
}

fn send_bytes(ep: Handle, buf_va: u64, socket: SocketId, bytes: &[u8]) -> bool {
    let mut offset = 0usize;
    while offset < bytes.len() {
        let count = (bytes.len() - offset).min(MAX_CLIENT_PAYLOAD_BYTES);
        unsafe {
            let destination =
                core::slice::from_raw_parts_mut((buf_va + PAYLOAD_OFFSET) as *mut u8, count);
            destination.copy_from_slice(&bytes[offset..offset + count]);
        }
        let (status, queued) = send_request(
            ep,
            buf_va,
            &SocketSend {
                socket_id: socket,
                byte_count: count as u32,
            },
        );
        if status != 0 || queued as usize != count {
            return false;
        }
        offset += count;
    }
    true
}

fn error_response(error: ParseError) -> (Status, &'static [u8]) {
    match error {
        ParseError::TooLarge | ParseError::TooManyHeaders => {
            (Status::RequestHeaderFieldsTooLarge, BAD_REQUEST_BODY)
        }
        ParseError::UnsupportedMethod => (Status::MethodNotAllowed, METHOD_BODY),
        _ => (Status::BadRequest, BAD_REQUEST_BODY),
    }
}

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

fn close_socket(ep: Handle, buf_va: u64, socket: SocketId) {
    let _ = send_request(ep, buf_va, &SocketClose { socket_id: socket });
}

fn send_request<T: Serialize>(ep: Handle, buf_va: u64, request: &T) -> (u64, u64) {
    let bytes = request.to_buffer().to_transfer_bytes();
    unsafe {
        core::slice::from_raw_parts_mut(buf_va as *mut u8, XFER_BYTES).copy_from_slice(&bytes);
    }
    let reply = sys::call(ep, 0, 0);
    (reply.val0, reply.val1)
}
