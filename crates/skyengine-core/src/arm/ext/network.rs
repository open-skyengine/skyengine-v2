use super::*;
use crate::wap_proxy::WAP_PROXY_ADDRESS;

const MAX_HTTP_HEADER_INSPECTION: usize = 64 * 1024;

#[derive(Debug, Eq, PartialEq)]
struct HttpAuthority {
    host: String,
    port: Option<u16>,
}

enum HttpRequestInspection {
    Incomplete,
    Direct,
    Routed(HttpAuthority),
}

fn parse_http_authority(value: &str) -> Option<HttpAuthority> {
    if !value.is_ascii() || value.starts_with('[') {
        return None;
    }
    let (host, port) = match value.matches(':').count() {
        0 => (value, None),
        1 => {
            let (host, port) = value.rsplit_once(':')?;
            (host, Some(port.parse().ok()?))
        }
        _ => return None,
    };
    let host = host.trim_end_matches('.');
    if host.is_empty()
        || host.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'/' | b'\\' | b'@' | b'#' | b'?')
        })
    {
        return None;
    }
    Some(HttpAuthority {
        host: host.to_ascii_lowercase(),
        port,
    })
}

fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
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

fn inspect_http_request(bytes: &[u8]) -> HttpRequestInspection {
    let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        let Some(line_end) = bytes.windows(2).position(|window| window == b"\r\n") else {
            return HttpRequestInspection::Direct;
        };
        let Ok(request_line) = std::str::from_utf8(&bytes[..line_end]) else {
            return HttpRequestInspection::Direct;
        };
        return if valid_http_request_line(request_line) {
            HttpRequestInspection::Incomplete
        } else {
            HttpRequestInspection::Direct
        };
    };
    let Ok(header) = std::str::from_utf8(&bytes[..header_end]) else {
        return HttpRequestInspection::Direct;
    };
    let mut lines = header.split("\r\n");
    if !lines.next().is_some_and(valid_http_request_line) {
        return HttpRequestInspection::Direct;
    }
    let mut authority = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return HttpRequestInspection::Direct;
        };
        if !is_http_token(name) {
            return HttpRequestInspection::Direct;
        }
        if name.eq_ignore_ascii_case("host") {
            if authority.is_some() {
                return HttpRequestInspection::Direct;
            }
            let Some(parsed) = parse_http_authority(value.trim_matches([' ', '\t'])) else {
                return HttpRequestInspection::Direct;
            };
            authority = Some(parsed);
        }
    }
    authority.map_or(HttpRequestInspection::Direct, HttpRequestInspection::Routed)
}

fn valid_http_request_line(line: &str) -> bool {
    if !line.is_ascii() {
        return false;
    }
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    is_http_token(method)
        && !target.is_empty()
        && !target.bytes().any(|byte| byte.is_ascii_control())
        && matches!(version, "HTTP/1.0" | "HTTP/1.1")
}

fn write_buffered_request(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    let write_result = stream
        .set_write_timeout(Some(NETWORK_CONNECT_TIMEOUT))
        .and_then(|()| stream.write_all(bytes));
    let timeout_result = stream.set_write_timeout(None);
    let nonblocking_result = stream.set_nonblocking(true);
    write_result.and(timeout_result).and(nonblocking_result)
}

impl ExtRuntime {
    pub(super) fn resolve_mapped_host(&self, name: &[u8]) -> Option<u32> {
        let name = std::str::from_utf8(name).ok()?.trim_end_matches('.');
        let mapping = self
            .dns_mappings
            .iter()
            .find(|mapping| mapping.source.eq_ignore_ascii_case(name))?;
        Some(u32::from_be_bytes(mapping.address.octets()))
    }

    pub(super) fn route_mapped_endpoint(&self, ip: u32, port: u32) -> (u32, u32) {
        let address = Ipv4Addr::from(ip.to_be_bytes());
        if let Some(mapping) = self.dns_mappings.iter().find(|mapping| {
            mapping
                .source
                .parse::<Ipv4Addr>()
                .is_ok_and(|source| source == address)
        }) {
            return (
                u32::from_be_bytes(mapping.address.octets()),
                mapping.port.map_or(port, u32::from),
            );
        }
        if address == WAP_PROXY_ADDRESS
            && let Some(endpoint) = self.wap_proxy_endpoint
        {
            return (
                u32::from_be_bytes(endpoint.ip().octets()),
                u32::from(endpoint.port()),
            );
        }
        if let Some(mapping) = self
            .dns_mappings
            .iter()
            .find(|mapping| mapping.port.is_some() && mapping.address == address)
        {
            return (ip, u32::from(mapping.port.unwrap()));
        }
        (ip, port)
    }

    pub(super) fn allocate_native_socket_handle(&mut self) -> Result<Option<i32>> {
        if self.native_sockets.len() >= MAX_NATIVE_SOCKETS {
            return Ok(None);
        }
        let start = self.next_native_socket_handle;
        loop {
            let handle = self.next_native_socket_handle;
            self.next_native_socket_handle = self
                .next_native_socket_handle
                .checked_add(1)
                .filter(|next| *next > 0)
                .unwrap_or(1);
            if let std::collections::btree_map::Entry::Vacant(entry) =
                self.native_sockets.entry(handle)
            {
                entry.insert(NativeSocket {
                    state: NativeSocketState::Created,
                    endpoint: None,
                    pending_http_request: Some(Vec::new()),
                    receive_mode: NativeSocketReceiveMode::BeforeSend,
                });
                return Ok(Some(handle));
            }
            if self.next_native_socket_handle == start {
                return Err(Error::ResourceLimit(
                    "no native socket handles are available".into(),
                ));
            }
        }
    }

    pub(super) fn connect_native_socket(
        &mut self,
        handle: i32,
        ip: u32,
        port: u32,
        mode: u32,
    ) -> i32 {
        let Some(socket) = self.native_sockets.get_mut(&handle) else {
            return -1;
        };
        match socket.state {
            NativeSocketState::Created => {}
            NativeSocketState::Connecting(_) => return 2,
            NativeSocketState::Connected(_) => return 0,
            NativeSocketState::Failed => return -1,
        }
        let Ok(port) = u16::try_from(port) else {
            socket.state = NativeSocketState::Failed;
            return -1;
        };
        if mode != 1 {
            socket.state = NativeSocketState::Failed;
            return -1;
        }
        let endpoint = SocketAddrV4::new(Ipv4Addr::from(ip.to_be_bytes()), port);
        let address = SocketAddr::V4(endpoint);
        let (sender, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(format!("skyengine-connect-{handle}"))
            .spawn(move || {
                let result = TcpStream::connect_timeout(&address, NETWORK_CONNECT_TIMEOUT);
                let _ = sender.send(result);
            });
        if worker.is_err() {
            socket.state = NativeSocketState::Failed;
            return -1;
        }
        socket.endpoint = Some(endpoint);
        socket.state = NativeSocketState::Connecting(receiver);
        2
    }

    pub(super) fn native_socket_state(&mut self, handle: i32) -> i32 {
        let Some(socket) = self.native_sockets.get_mut(&handle) else {
            return -1;
        };
        let completed = match &socket.state {
            NativeSocketState::Connecting(receiver) => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    socket.state = NativeSocketState::Failed;
                    return -1;
                }
            },
            _ => None,
        };
        if let Some(result) = completed {
            socket.state = match result {
                Ok(stream) if stream.set_nonblocking(true).is_ok() => {
                    NativeSocketState::Connected(stream)
                }
                Ok(_) | Err(_) => NativeSocketState::Failed,
            };
        }
        match socket.state {
            NativeSocketState::Created | NativeSocketState::Connecting(_) => 1,
            NativeSocketState::Connected(_) => 0,
            NativeSocketState::Failed => -1,
        }
    }

    pub(super) fn receive_native_socket(&mut self, handle: i32, len: usize) -> Option<Vec<u8>> {
        if len > self.heap_len || self.native_socket_state(handle) != 0 {
            return None;
        }
        let socket = self.native_sockets.get_mut(&handle)?;
        let NativeSocketState::Connected(stream) = &mut socket.state else {
            return None;
        };
        let mut bytes = vec![0; len];
        let deadline = (socket.receive_mode == NativeSocketReceiveMode::WaitForFirstResponse)
            .then(|| Instant::now() + NETWORK_FIRST_RECEIVE_TIMEOUT);
        socket.receive_mode = NativeSocketReceiveMode::Polling;
        loop {
            match stream.read(&mut bytes) {
                Ok(read) => {
                    bytes.truncate(read);
                    return Some(bytes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if deadline.is_some_and(|deadline| Instant::now() < deadline) {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    return None;
                }
                Err(_) => {
                    socket.state = NativeSocketState::Failed;
                    return None;
                }
            }
        }
    }

    pub(super) fn send_native_socket(&mut self, handle: i32, bytes: &[u8]) -> Option<usize> {
        if bytes.len() > self.heap_len || self.native_socket_state(handle) != 0 {
            return None;
        }
        let socket = self.native_sockets.get_mut(&handle)?;
        if socket.pending_http_request.is_none() {
            let NativeSocketState::Connected(stream) = &mut socket.state else {
                return None;
            };
            let written = stream.write(bytes).ok();
            if written.is_some_and(|written| written != 0)
                && socket.receive_mode == NativeSocketReceiveMode::BeforeSend
            {
                socket.receive_mode = NativeSocketReceiveMode::WaitForFirstResponse;
            }
            return written;
        }

        let pending = socket.pending_http_request.as_mut()?;
        let combined_len = pending.len().checked_add(bytes.len())?;
        if combined_len > self.heap_len {
            socket.state = NativeSocketState::Failed;
            return None;
        }
        pending.extend_from_slice(bytes);
        let inspection = if pending.len() > MAX_HTTP_HEADER_INSPECTION {
            HttpRequestInspection::Direct
        } else {
            inspect_http_request(pending)
        };
        if matches!(inspection, HttpRequestInspection::Incomplete) {
            return Some(bytes.len());
        }

        let buffered = socket.pending_http_request.take()?;
        let endpoint = socket.endpoint?;
        let target = match inspection {
            HttpRequestInspection::Routed(authority) => self
                .dns_mappings
                .iter()
                .find(|mapping| mapping.source.eq_ignore_ascii_case(&authority.host))
                .map(|mapping| {
                    SocketAddrV4::new(
                        mapping.address,
                        mapping.port.or(authority.port).unwrap_or(endpoint.port()),
                    )
                }),
            HttpRequestInspection::Incomplete => unreachable!(),
            HttpRequestInspection::Direct => None,
        };

        if target.is_none() || target == Some(endpoint) {
            let socket = self.native_sockets.get_mut(&handle)?;
            let NativeSocketState::Connected(stream) = &mut socket.state else {
                return None;
            };
            if write_buffered_request(stream, &buffered).is_err() {
                socket.state = NativeSocketState::Failed;
                return None;
            }
            if socket.receive_mode == NativeSocketReceiveMode::BeforeSend {
                socket.receive_mode = NativeSocketReceiveMode::WaitForFirstResponse;
            }
            return Some(bytes.len());
        }

        let target = SocketAddr::V4(target?);
        let replacement =
            TcpStream::connect_timeout(&target, NETWORK_CONNECT_TIMEOUT).and_then(|mut stream| {
                write_buffered_request(&mut stream, &buffered)?;
                Ok(stream)
            });
        let socket = self.native_sockets.get_mut(&handle)?;
        match replacement {
            Ok(stream) => {
                socket.endpoint = match target {
                    SocketAddr::V4(endpoint) => Some(endpoint),
                    SocketAddr::V6(_) => unreachable!(),
                };
                socket.state = NativeSocketState::Connected(stream);
                if socket.receive_mode == NativeSocketReceiveMode::BeforeSend {
                    socket.receive_mode = NativeSocketReceiveMode::WaitForFirstResponse;
                }
            }
            Err(_) => {
                socket.state = NativeSocketState::Failed;
                return None;
            }
        }
        Some(bytes.len())
    }
}
