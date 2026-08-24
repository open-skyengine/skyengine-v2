use std::{
    io::{ErrorKind, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs},
    sync::mpsc,
    thread,
    time::Duration,
};

use super::*;

const MAX_MR_SOCKETS: usize = 64;
const NETWORK_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CONNECT_HEADER_INSPECTION: usize = 4 * 1024;
const CONNECT_PREFIX: &[u8] = b"CONNECT ";

#[derive(Debug)]
enum MrSocketState {
    Created,
    Connecting(mpsc::Receiver<std::io::Result<TcpStream>>),
    Connected(TcpStream),
    Failed,
    Closed,
}

#[derive(Debug)]
pub(super) struct MrSocket {
    state: MrSocketState,
    connect_port_route: Option<ConnectPortRoute>,
}

#[derive(Debug)]
struct ConnectPortRoute {
    address: Ipv4Addr,
    pending: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RoutedMrEndpoint {
    endpoint: SocketAddrV4,
    connect_port_address: Option<Ipv4Addr>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MrSocketSendResult {
    Accepted(usize),
    WouldBlock,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectHeaderInspection {
    Incomplete,
    Direct,
    Routed(u16),
}

impl MrSocket {
    fn send(&mut self, input: &[u8]) -> MrSocketSendResult {
        let Some(route) = self.connect_port_route.as_ref() else {
            return self.write_direct(input);
        };

        if !combined_starts_with(route.pending.as_slice(), input, CONNECT_PREFIX) {
            let pending = self.connect_port_route.take().unwrap().pending;
            if pending.is_empty() {
                return self.write_direct(input);
            }
            return self.write_accepted(&pending, input, input.len());
        }

        let (inspection, buffered_from_input) = {
            let route = self.connect_port_route.as_mut().unwrap();
            let available = MAX_CONNECT_HEADER_INSPECTION.saturating_sub(route.pending.len());
            let buffered_from_input = input.len().min(available);
            route
                .pending
                .extend_from_slice(&input[..buffered_from_input]);
            (inspect_connect_header(&route.pending), buffered_from_input)
        };

        if inspection == ConnectHeaderInspection::Incomplete {
            if buffered_from_input == input.len() {
                return MrSocketSendResult::Accepted(input.len());
            }
            self.state = MrSocketState::Failed;
            return MrSocketSendResult::Failed;
        }

        let route = self.connect_port_route.take().unwrap();
        let remaining = &input[buffered_from_input..];
        match inspection {
            ConnectHeaderInspection::Incomplete => unreachable!(),
            ConnectHeaderInspection::Direct => {
                self.write_accepted(&route.pending, remaining, input.len())
            }
            ConnectHeaderInspection::Routed(port) => {
                let target = SocketAddr::V4(SocketAddrV4::new(route.address, port));
                let replacement = TcpStream::connect_timeout(&target, NETWORK_CONNECT_TIMEOUT)
                    .and_then(|mut stream| {
                        write_buffered_input(&mut stream, &route.pending, remaining)?;
                        Ok(stream)
                    });
                match replacement {
                    Ok(stream) => {
                        self.state = MrSocketState::Connected(stream);
                        MrSocketSendResult::Accepted(input.len())
                    }
                    Err(_) => {
                        self.state = MrSocketState::Failed;
                        MrSocketSendResult::Failed
                    }
                }
            }
        }
    }

    fn write_direct(&mut self, input: &[u8]) -> MrSocketSendResult {
        let result = match &mut self.state {
            MrSocketState::Connected(stream) => stream.write(input),
            _ => return MrSocketSendResult::Failed,
        };
        match result {
            Ok(sent) => MrSocketSendResult::Accepted(sent),
            Err(error) if error.kind() == ErrorKind::WouldBlock => MrSocketSendResult::WouldBlock,
            Err(_) => {
                self.state = MrSocketState::Failed;
                MrSocketSendResult::Failed
            }
        }
    }

    fn write_accepted(
        &mut self,
        buffered: &[u8],
        remaining: &[u8],
        accepted: usize,
    ) -> MrSocketSendResult {
        let result = match &mut self.state {
            MrSocketState::Connected(stream) => write_buffered_input(stream, buffered, remaining),
            _ => return MrSocketSendResult::Failed,
        };
        match result {
            Ok(()) => MrSocketSendResult::Accepted(accepted),
            Err(_) => {
                self.state = MrSocketState::Failed;
                MrSocketSendResult::Failed
            }
        }
    }
}

pub(super) fn socket_library() -> TableRef {
    let socket = Table::new();
    let mut values = socket.borrow_mut();
    values.set(bytes(b"state"), Value::Number(3.0));
    values.set(bytes(b"ip"), Value::Number(-1.0));
    values.set(bytes(b"tcp"), Value::Native("socket_tcp"));
    drop(values);
    socket
}

impl MrHost {
    pub(crate) fn socket_library(&self) -> TableRef {
        self.socket_library.clone()
    }

    pub(super) fn test_com1(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let command = integer(args.first())?;
        if command != 100 {
            return Err(crate::Error::Platform(format!(
                "unsupported TestCom1 command {command}"
            )));
        }
        let name = value_bytes(args.get(1))?;
        let name = std::str::from_utf8(&name)
            .map_err(|_| crate::Error::MrFault("TestCom1 host is not UTF-8".into()))?
            .trim_end_matches('.');
        let address = self
            .dns_mappings
            .iter()
            .find(|mapping| mapping.source.eq_ignore_ascii_case(name))
            .map(|mapping| mapping.address)
            .or_else(|| {
                (name, 0)
                    .to_socket_addrs()
                    .ok()?
                    .find_map(|address| match address {
                        SocketAddr::V4(address) => Some(*address.ip()),
                        SocketAddr::V6(_) => None,
                    })
            });
        let ip = address.map_or(-1.0, |address| {
            f64::from(u32::from_be_bytes(address.octets()))
        });
        self.socket_library
            .borrow_mut()
            .set(bytes(b"ip"), Value::Number(ip));
        Ok(vec![Value::Number(ip)])
    }

    pub(super) fn close_network(&mut self) -> Result<Vec<Value>> {
        self.mr_sockets.clear();
        self.next_mr_socket_handle = 1;
        let mut socket = self.socket_library.borrow_mut();
        socket.set(bytes(b"state"), Value::Number(3.0));
        socket.set(bytes(b"ip"), Value::Number(-1.0));
        Ok(vec![Value::Number(0.0)])
    }

    pub(super) fn initialize_network(&mut self) {
        self.socket_library
            .borrow_mut()
            .set(bytes(b"state"), Value::Number(2.0));
    }

    pub(super) fn socket_tcp(&mut self) -> Result<Vec<Value>> {
        if self.mr_sockets.len() >= MAX_MR_SOCKETS {
            return Ok(vec![Value::Nil]);
        }
        let start = self.next_mr_socket_handle;
        let handle = loop {
            let handle = self.next_mr_socket_handle;
            self.next_mr_socket_handle = self
                .next_mr_socket_handle
                .checked_add(1)
                .filter(|next| *next > 0)
                .unwrap_or(1);
            if !self.mr_sockets.contains_key(&handle) {
                break handle;
            }
            if self.next_mr_socket_handle == start {
                return Err(crate::Error::ResourceLimit(
                    "no MR socket handles are available".into(),
                ));
            }
        };
        self.mr_sockets.insert(
            handle,
            MrSocket {
                state: MrSocketState::Created,
                connect_port_route: None,
            },
        );

        let socket = Table::new();
        let mut values = socket.borrow_mut();
        values.set(bytes(b"__handle"), Value::Number(f64::from(handle)));
        values.set(bytes(b"state"), Value::Number(1.0));
        for (name, native) in [
            (b"connect".as_slice(), "socket_connect"),
            (b"getstate".as_slice(), "socket_getstate"),
            (b"getinfo".as_slice(), "socket_getinfo"),
            (b"send".as_slice(), "socket_send"),
            (b"receive".as_slice(), "socket_receive"),
            (b"close".as_slice(), "socket_close"),
        ] {
            values.set(bytes(name), Value::Native(native));
        }
        drop(values);
        Ok(vec![Value::Table(socket)])
    }

    pub(super) fn socket_connect(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = socket_handle(args.first())?;
        let ip = socket_ip(args.get(1))?;
        let port = u16::try_from(integer(args.get(2))?)
            .map_err(|_| crate::Error::MrFault("socket port is out of range".into()))?;
        let route = route_mr_endpoint(&self.dns_mappings, ip, port);
        let Some(socket) = self.mr_sockets.get_mut(&handle) else {
            return Ok(vec![Value::Number(5.0)]);
        };
        match socket.state {
            MrSocketState::Created => {}
            MrSocketState::Connecting(_) => return Ok(vec![Value::Number(2.0)]),
            MrSocketState::Connected(_) => return Ok(vec![Value::Number(0.0)]),
            MrSocketState::Failed | MrSocketState::Closed => {
                return Ok(vec![Value::Number(5.0)]);
            }
        }

        let address = SocketAddr::V4(route.endpoint);
        let (sender, receiver) = mpsc::channel();
        if thread::Builder::new()
            .name(format!("skyengine-mr-connect-{handle}"))
            .spawn(move || {
                let result = TcpStream::connect_timeout(&address, NETWORK_CONNECT_TIMEOUT);
                let _ = sender.send(result);
            })
            .is_err()
        {
            socket.state = MrSocketState::Failed;
            return Ok(vec![Value::Number(5.0)]);
        }
        socket.connect_port_route = route.connect_port_address.map(|address| ConnectPortRoute {
            address,
            pending: Vec::new(),
        });
        socket.state = MrSocketState::Connecting(receiver);
        Ok(vec![Value::Number(2.0)])
    }

    pub(super) fn socket_get_state(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = socket_handle(args.first())?;
        let state = self.poll_mr_socket(handle);
        if let Some(Value::Table(table)) = args.first() {
            table
                .borrow_mut()
                .set(bytes(b"state"), Value::Number(f64::from(state)));
        }
        Ok(vec![Value::Number(f64::from(state))])
    }

    pub(super) fn socket_get_info(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = socket_handle(args.first())?;
        let state = self.poll_mr_socket(handle);
        let socket = args.first().cloned().unwrap_or(Value::Nil);
        if let Value::Table(table) = &socket {
            table
                .borrow_mut()
                .set(bytes(b"state"), Value::Number(f64::from(state)));
        }
        Ok(vec![socket])
    }

    pub(super) fn socket_send(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = socket_handle(args.first())?;
        let input = value_bytes(args.get(1))?;
        if self.poll_mr_socket(handle) != 2 {
            return Ok(vec![Value::Number(-1.0)]);
        }
        let Some(socket) = self.mr_sockets.get_mut(&handle) else {
            return Ok(vec![Value::Number(-1.0)]);
        };
        let sent = match socket.send(&input) {
            MrSocketSendResult::Accepted(sent) => sent as f64,
            MrSocketSendResult::WouldBlock => 0.0,
            MrSocketSendResult::Failed => -1.0,
        };
        Ok(vec![Value::Number(sent)])
    }

    pub(super) fn socket_receive(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = socket_handle(args.first())?;
        let len = usize::try_from(integer(args.get(1))?).unwrap_or(0);
        if self.poll_mr_socket(handle) != 2 {
            return Ok(vec![Value::Number(-1.0)]);
        }
        let Some(MrSocket {
            state: MrSocketState::Connected(stream),
            ..
        }) = self.mr_sockets.get_mut(&handle)
        else {
            return Ok(vec![Value::Number(-1.0)]);
        };
        let mut output = vec![0; len];
        match stream.read(&mut output) {
            Ok(0) => Ok(vec![Value::Number(-1.0)]),
            Ok(read) => {
                output.truncate(read);
                Ok(vec![Value::Bytes(output.into())])
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(vec![Value::Number(0.0)]),
            Err(_) => {
                self.mr_sockets.get_mut(&handle).unwrap().state = MrSocketState::Failed;
                Ok(vec![Value::Number(-1.0)])
            }
        }
    }

    pub(super) fn socket_close(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = socket_handle(args.first())?;
        if let Some(socket) = self.mr_sockets.get_mut(&handle) {
            socket.state = MrSocketState::Closed;
        }
        Ok(vec![Value::Number(0.0)])
    }

    fn poll_mr_socket(&mut self, handle: i32) -> i32 {
        let Some(socket) = self.mr_sockets.get_mut(&handle) else {
            return 5;
        };
        let completion = match &socket.state {
            MrSocketState::Connecting(receiver) => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    socket.state = MrSocketState::Failed;
                    return 5;
                }
            },
            _ => None,
        };
        if let Some(result) = completion {
            socket.state = match result {
                Ok(stream) if stream.set_nonblocking(true).is_ok() => {
                    MrSocketState::Connected(stream)
                }
                Ok(_) | Err(_) => MrSocketState::Failed,
            };
        }
        match socket.state {
            MrSocketState::Created | MrSocketState::Connecting(_) => 1,
            MrSocketState::Connected(_) => 2,
            MrSocketState::Failed => 5,
            MrSocketState::Closed => 3,
        }
    }
}

fn route_mr_endpoint(mappings: &[DnsMapping], ip: Ipv4Addr, port: u16) -> RoutedMrEndpoint {
    if let Some(mapping) = mappings.iter().find(|mapping| {
        mapping
            .source
            .parse::<Ipv4Addr>()
            .is_ok_and(|source| source == ip)
    }) {
        return RoutedMrEndpoint {
            endpoint: SocketAddrV4::new(mapping.address, mapping.port.unwrap_or(port)),
            connect_port_address: mapping.port.map(|_| mapping.address),
        };
    }
    if let Some(mapping) = mappings
        .iter()
        .find(|mapping| mapping.port.is_some() && mapping.address == ip)
    {
        return RoutedMrEndpoint {
            endpoint: SocketAddrV4::new(ip, mapping.port.unwrap()),
            connect_port_address: None,
        };
    }
    RoutedMrEndpoint {
        endpoint: SocketAddrV4::new(ip, port),
        connect_port_address: None,
    }
}

fn combined_starts_with(first: &[u8], second: &[u8], prefix: &[u8]) -> bool {
    let combined_len = first.len().saturating_add(second.len());
    let compared_len = combined_len.min(prefix.len());
    (0..compared_len).all(|index| {
        let byte = if index < first.len() {
            first[index]
        } else {
            second[index - first.len()]
        };
        byte == prefix[index]
    })
}

fn inspect_connect_header(bytes: &[u8]) -> ConnectHeaderInspection {
    let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return ConnectHeaderInspection::Incomplete;
    };
    let Some(line_end) = bytes[..header_end + 2]
        .windows(2)
        .position(|window| window == b"\r\n")
    else {
        return ConnectHeaderInspection::Direct;
    };
    let Ok(request_line) = std::str::from_utf8(&bytes[..line_end]) else {
        return ConnectHeaderInspection::Direct;
    };
    let mut parts = request_line.split(' ');
    let (Some("CONNECT"), Some(authority), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return ConnectHeaderInspection::Direct;
    };
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return ConnectHeaderInspection::Direct;
    }
    let Some((host, port)) = authority.rsplit_once(':') else {
        return ConnectHeaderInspection::Direct;
    };
    if host.is_empty()
        || !host.is_ascii()
        || host.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'/' | b'\\' | b'@' | b'#' | b'?')
        })
    {
        return ConnectHeaderInspection::Direct;
    }
    port.parse().map_or(
        ConnectHeaderInspection::Direct,
        ConnectHeaderInspection::Routed,
    )
}

fn write_buffered_input(
    stream: &mut TcpStream,
    buffered: &[u8],
    remaining: &[u8],
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    let write_result = stream
        .set_write_timeout(Some(NETWORK_CONNECT_TIMEOUT))
        .and_then(|()| stream.write_all(buffered))
        .and_then(|()| stream.write_all(remaining));
    let timeout_result = stream.set_write_timeout(None);
    let nonblocking_result = stream.set_nonblocking(true);
    write_result.and(timeout_result).and(nonblocking_result)
}

fn socket_handle(value: Option<&Value>) -> Result<i32> {
    let Some(Value::Table(socket)) = value else {
        return Err(crate::Error::MrFault(
            "socket method expects its receiver".into(),
        ));
    };
    let handle = socket.borrow().get(&bytes(b"__handle"));
    integer(Some(&handle))
}

fn socket_ip(value: Option<&Value>) -> Result<Ipv4Addr> {
    match value {
        Some(Value::Number(ip)) if *ip >= 0.0 && *ip <= f64::from(u32::MAX) => {
            Ok(Ipv4Addr::from((*ip as u32).to_be_bytes()))
        }
        Some(value) => {
            let bytes = value
                .bytes()
                .ok_or_else(|| crate::Error::MrFault("socket IP is invalid".into()))?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| crate::Error::MrFault("socket IP is not UTF-8".into()))?;
            text.parse()
                .map_err(|_| crate::Error::MrFault(format!("invalid socket IP {text}")))
        }
        None => Err(crate::Error::MrFault("socket IP is missing".into())),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };

    use super::*;

    fn accept_before(listener: TcpListener) -> TcpStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    // Accepted sockets inherit nonblocking mode on Windows but
                    // not on every Unix platform. Server-side test reads below
                    // require the same blocking behavior on every host.
                    stream.set_nonblocking(false).unwrap();
                    return stream;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "TCP accept timed out");
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("TCP accept failed: {error}"),
            }
        }
    }

    fn mapped_socket(route: RoutedMrEndpoint) -> MrSocket {
        let stream = TcpStream::connect(SocketAddr::V4(route.endpoint)).unwrap();
        stream.set_nonblocking(true).unwrap();
        MrSocket {
            state: MrSocketState::Connected(stream),
            connect_port_route: route.connect_port_address.map(|address| ConnectPortRoute {
                address,
                pending: Vec::new(),
            }),
        }
    }

    #[test]
    fn connect_port_routing_requires_an_ip_source_with_a_fixed_port() {
        let source = Ipv4Addr::new(10, 0, 0, 172);
        let fixed = DnsMapping {
            source: source.to_string(),
            address: Ipv4Addr::LOCALHOST,
            port: Some(13_230),
        };
        assert_eq!(
            route_mr_endpoint(std::slice::from_ref(&fixed), source, 80),
            RoutedMrEndpoint {
                endpoint: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 13_230),
                connect_port_address: Some(Ipv4Addr::LOCALHOST),
            }
        );

        let without_port = DnsMapping {
            port: None,
            ..fixed.clone()
        };
        assert_eq!(
            route_mr_endpoint(&[without_port], source, 80).connect_port_address,
            None
        );

        let hostname_source = DnsMapping {
            source: "proxy.test".into(),
            ..fixed
        };
        let hostname_route = route_mr_endpoint(&[hostname_source], Ipv4Addr::LOCALHOST, 80);
        assert_eq!(hostname_route.endpoint.port(), 13_230);
        assert_eq!(hostname_route.connect_port_address, None);
    }

    #[test]
    fn split_connect_header_reconnects_to_its_port_on_the_mapped_address() {
        let initial_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let initial_port = initial_listener.local_addr().unwrap().port();
        let initial_server = thread::spawn(move || {
            let mut stream = accept_before(initial_listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut received = Vec::new();
            stream.read_to_end(&mut received).unwrap();
            received
        });

        let target_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let target_port = target_listener.local_addr().unwrap().port();
        let request = format!(
            "CONNECT wap.skmeg.com:{target_port} HTTP/1.1\r\n\
             Proxy-Connection: Keep-Alive\r\n\r\n"
        )
        .into_bytes();
        let expected = request.clone();
        let target_server = thread::spawn(move || {
            let mut stream = accept_before(target_listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut received = vec![0; expected.len()];
            stream.read_exact(&mut received).unwrap();
            stream.write_all(b"ok").unwrap();
            (received, expected)
        });

        let source = Ipv4Addr::new(10, 0, 0, 172);
        let mapping = DnsMapping {
            source: source.to_string(),
            address: Ipv4Addr::LOCALHOST,
            port: Some(initial_port),
        };
        let route = route_mr_endpoint(&[mapping], source, 80);
        let mut socket = mapped_socket(route);
        let request_line_end = request
            .windows(2)
            .position(|window| window == b"\r\n")
            .unwrap();
        let splits = [3, request_line_end + 1, request.len()];
        let mut start = 0;
        for end in splits {
            assert_eq!(
                socket.send(&request[start..end]),
                MrSocketSendResult::Accepted(end - start)
            );
            start = end;
        }

        let (received, expected) = target_server.join().unwrap();
        assert_eq!(received, expected);
        assert!(initial_server.join().unwrap().is_empty());
    }

    #[test]
    fn ordinary_first_send_stays_on_the_initial_mapped_connection() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let mut stream = accept_before(listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut received = [0; 4];
            stream.read_exact(&mut received).unwrap();
            received
        });

        let source = Ipv4Addr::new(10, 0, 0, 172);
        let mapping = DnsMapping {
            source: source.to_string(),
            address: Ipv4Addr::LOCALHOST,
            port: Some(port),
        };
        let route = route_mr_endpoint(&[mapping], source, 80);
        let mut socket = mapped_socket(route);

        assert_eq!(socket.send(b"ping"), MrSocketSendResult::Accepted(4));
        assert_eq!(server.join().unwrap(), *b"ping");
        assert!(socket.connect_port_route.is_none());
    }

    #[test]
    fn failed_connect_port_replacement_marks_the_socket_failed() {
        let initial_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let initial_port = initial_listener.local_addr().unwrap().port();
        let initial_server = thread::spawn(move || {
            let mut stream = accept_before(initial_listener);
            stream
                .set_read_timeout(Some(NETWORK_CONNECT_TIMEOUT + Duration::from_secs(2)))
                .unwrap();
            let mut received = Vec::new();
            stream.read_to_end(&mut received).unwrap();
            received
        });
        let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let unavailable_port = unavailable.local_addr().unwrap().port();
        drop(unavailable);

        let source = Ipv4Addr::new(10, 0, 0, 172);
        let mapping = DnsMapping {
            source: source.to_string(),
            address: Ipv4Addr::LOCALHOST,
            port: Some(initial_port),
        };
        let route = route_mr_endpoint(&[mapping], source, 80);
        let mut socket = mapped_socket(route);
        let request = format!("CONNECT ignored.test:{unavailable_port} HTTP/1.0\r\n\r\n");

        assert_eq!(socket.send(request.as_bytes()), MrSocketSendResult::Failed);
        assert!(matches!(socket.state, MrSocketState::Failed));
        assert!(initial_server.join().unwrap().is_empty());
    }
}
