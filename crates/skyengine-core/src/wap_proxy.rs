use std::{
    io::{self, ErrorKind, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream, ToSocketAddrs},
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::DnsMapping;

pub(crate) const WAP_PROXY_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 172);

const MAX_REQUEST_HEADER_LEN: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) struct WapProxyService {
    endpoint: SocketAddrV4,
    shutdown: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl WapProxyService {
    pub(crate) fn start(dns_mappings: Arc<[DnsMapping]>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let endpoint = match listener.local_addr()? {
            SocketAddr::V4(endpoint) => endpoint,
            SocketAddr::V6(_) => unreachable!("the WAP proxy binds an IPv4 listener"),
        };
        let (shutdown, shutdown_receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("skyengine-wap-proxy".into())
            .spawn(move || run_service(listener, dns_mappings, shutdown_receiver))?;
        Ok(Self {
            endpoint,
            shutdown,
            worker: Some(worker),
        })
    }

    pub(crate) fn endpoint(&self) -> SocketAddrV4 {
        self.endpoint
    }
}

impl Drop for WapProxyService {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_service(
    listener: TcpListener,
    dns_mappings: Arc<[DnsMapping]>,
    shutdown: mpsc::Receiver<()>,
) {
    loop {
        if shutdown.try_recv().is_ok() {
            return;
        }
        match listener.accept() {
            Ok((client, _)) => {
                let dns_mappings = dns_mappings.clone();
                let _ = thread::Builder::new()
                    .name("skyengine-wap-client".into())
                    .spawn(move || {
                        let _ = serve_client(client, &dns_mappings);
                    });
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                match shutdown.recv_timeout(ACCEPT_POLL_INTERVAL) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Authority {
    host: String,
    port: u16,
}

enum ProxyRequest {
    Connect(Authority),
    Forward {
        authority: Authority,
        header: Vec<u8>,
    },
}

fn serve_client(mut client: TcpStream, dns_mappings: &[DnsMapping]) -> io::Result<()> {
    let (buffered, body_offset) = match read_request_header(&mut client) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_error_response(&mut client, 400, "Bad Request");
            return Err(error);
        }
    };
    let request = match parse_request(&buffered[..body_offset - 4]) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_error_response(&mut client, 400, "Bad Request");
            return Err(error);
        }
    };
    let authority = match &request {
        ProxyRequest::Connect(authority) => authority,
        ProxyRequest::Forward { authority, .. } => authority,
    };
    let mut upstream = match connect_target(authority, dns_mappings) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = write_error_response(&mut client, 502, "Bad Gateway");
            return Err(error);
        }
    };
    let buffered_body = &buffered[body_offset..];

    match request {
        ProxyRequest::Connect(_) => {
            client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
            upstream.write_all(buffered_body)?;
        }
        ProxyRequest::Forward { header, .. } => {
            upstream.write_all(&header)?;
            upstream.write_all(buffered_body)?;
        }
    }
    relay(client, upstream)
}

fn read_request_header(stream: &mut TcpStream) -> io::Result<(Vec<u8>, usize)> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(header_end) = find_bytes(&bytes, b"\r\n\r\n") {
            return Ok((bytes, header_end + 4));
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "WAP proxy connection closed before the request header",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
        if find_bytes(&bytes, b"\r\n\r\n").is_none() && bytes.len() > MAX_REQUEST_HEADER_LEN {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "WAP proxy request header is too large",
            ));
        }
    }
}

fn parse_request(header: &[u8]) -> io::Result<ProxyRequest> {
    let lines = split_header_lines(header);
    let request_line = lines.first().ok_or_else(invalid_request)?;
    let request_line = std::str::from_utf8(request_line).map_err(|_| invalid_request())?;
    if !request_line.is_ascii() {
        return Err(invalid_request());
    }
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid_request());
    };
    if !is_http_token(method) || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(invalid_request());
    }
    if target.is_empty() || target.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(invalid_request());
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        return Ok(ProxyRequest::Connect(parse_authority(target, None)?));
    }

    let mut host = None;
    let mut forwarded_headers = Vec::new();
    for line in &lines[1..] {
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(invalid_request());
        };
        let name = std::str::from_utf8(&line[..colon]).map_err(|_| invalid_request())?;
        if !is_http_token(name) {
            return Err(invalid_request());
        }
        if name.eq_ignore_ascii_case("host") {
            if host.is_some() {
                return Err(invalid_request());
            }
            let value = trim_ascii_whitespace(&line[colon + 1..]);
            let value = std::str::from_utf8(value).map_err(|_| invalid_request())?;
            host = Some(parse_authority(value, Some(80))?);
        }
        if !name.eq_ignore_ascii_case("proxy-connection") {
            forwarded_headers.extend_from_slice(line);
            forwarded_headers.extend_from_slice(b"\r\n");
        }
    }

    let (authority, origin_target, add_host) = match parse_absolute_http_target(target)? {
        Some((authority, origin_target)) => {
            let add_host = host.is_none();
            (authority, origin_target, add_host)
        }
        None if target.starts_with('/') || target == "*" => {
            (host.ok_or_else(invalid_request)?, target.to_owned(), false)
        }
        None => return Err(invalid_request()),
    };

    let mut forwarded = format!("{method} {origin_target} {version}\r\n").into_bytes();
    forwarded.extend_from_slice(&forwarded_headers);
    if add_host {
        forwarded.extend_from_slice(b"Host: ");
        forwarded.extend_from_slice(authority.host.as_bytes());
        if authority.port != 80 {
            forwarded.extend_from_slice(format!(":{}", authority.port).as_bytes());
        }
        forwarded.extend_from_slice(b"\r\n");
    }
    forwarded.extend_from_slice(b"\r\n");
    Ok(ProxyRequest::Forward {
        authority,
        header: forwarded,
    })
}

fn parse_absolute_http_target(target: &str) -> io::Result<Option<(Authority, String)>> {
    let Some(scheme) = target.get(..7) else {
        return Ok(None);
    };
    if !scheme.eq_ignore_ascii_case("http://") {
        return Ok(None);
    }
    let rest = &target[7..];
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = parse_authority(&rest[..split], Some(80))?;
    let origin_target = match rest.get(split..) {
        Some(path) if path.starts_with('?') => format!("/{path}"),
        Some("") | None => "/".to_owned(),
        Some(path) => path.to_owned(),
    };
    Ok(Some((authority, origin_target)))
}

fn parse_authority(value: &str, default_port: Option<u16>) -> io::Result<Authority> {
    if !value.is_ascii() || value.starts_with('[') {
        return Err(invalid_request());
    }
    let (host, port) = match value.matches(':').count() {
        0 => (value, default_port.ok_or_else(invalid_request)?),
        1 => {
            let (host, port) = value.rsplit_once(':').ok_or_else(invalid_request)?;
            let port = port.parse::<u16>().map_err(|_| invalid_request())?;
            (host, port)
        }
        _ => return Err(invalid_request()),
    };
    let host = host.trim_end_matches('.');
    if host.is_empty()
        || host.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'/' | b'\\' | b'@' | b'#' | b'?')
        })
    {
        return Err(invalid_request());
    }
    Ok(Authority {
        host: host.to_ascii_lowercase(),
        port,
    })
}

fn connect_target(authority: &Authority, dns_mappings: &[DnsMapping]) -> io::Result<TcpStream> {
    if let Some(mapping) = dns_mappings.iter().find(|mapping| {
        mapping.source.eq_ignore_ascii_case(&authority.host)
            || authority
                .host
                .parse::<Ipv4Addr>()
                .ok()
                .is_some_and(|address| mapping.source.parse::<Ipv4Addr>() == Ok(address))
    }) {
        let endpoint = SocketAddr::V4(SocketAddrV4::new(
            mapping.address,
            mapping.port.unwrap_or(authority.port),
        ));
        return TcpStream::connect_timeout(&endpoint, CONNECT_TIMEOUT);
    }

    let addresses = (authority.host.as_str(), authority.port).to_socket_addrs()?;
    let mut last_error = None;
    for address in addresses.filter(|address| address.is_ipv4()) {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            ErrorKind::AddrNotAvailable,
            format!("no IPv4 address is available for {}", authority.host),
        )
    }))
}

fn relay(mut client: TcpStream, mut upstream: TcpStream) -> io::Result<()> {
    let mut client_reader = client.try_clone()?;
    let mut upstream_writer = upstream.try_clone()?;
    let upload = thread::Builder::new()
        .name("skyengine-wap-upload".into())
        .spawn(move || {
            let result = io::copy(&mut client_reader, &mut upstream_writer);
            let _ = upstream_writer.shutdown(Shutdown::Write);
            result
        })?;

    let download = io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    let upload = upload
        .join()
        .map_err(|_| io::Error::other("WAP proxy upload worker panicked"))?;
    download.and(upload).map(|_| ())
}

fn write_error_response(stream: &mut TcpStream, status: u16, reason: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    )
}

fn split_header_lines(mut header: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    loop {
        let Some(end) = find_bytes(header, b"\r\n") else {
            lines.push(header);
            return lines;
        };
        lines.push(&header[..end]);
        header = &header[end + 2..];
    }
}

fn find_bytes(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
}

fn trim_ascii_whitespace(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
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

fn invalid_request() -> io::Error {
    io::Error::new(ErrorKind::InvalidData, "invalid WAP proxy request")
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    use super::*;

    fn mapping(source: &str, listener: &TcpListener) -> DnsMapping {
        DnsMapping {
            source: source.into(),
            address: Ipv4Addr::LOCALHOST,
            port: Some(listener.local_addr().unwrap().port()),
        }
    }

    #[test]
    fn forwards_absolute_http_requests_to_mapped_hosts() {
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mapping = mapping("service.test", &target);
        let expected = b"GET /resource?id=7 HTTP/1.1\r\nHost: service.test\r\n\r\n";
        let target_worker = thread::spawn(move || {
            let (mut stream, _) = target.accept().unwrap();
            let mut request = vec![0; expected.len()];
            stream.read_exact(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
            request
        });
        let service = WapProxyService::start(vec![mapping].into()).unwrap();
        let mut client = TcpStream::connect(service.endpoint()).unwrap();
        client
            .write_all(
                b"GET http://service.test/resource?id=7 HTTP/1.1\r\nHost: service.test\r\nProxy-Connection: Keep-Alive\r\n\r\n",
            )
            .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();

        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert_eq!(target_worker.join().unwrap(), expected);
    }

    #[test]
    fn connect_establishes_a_tunnel_and_relays_buffered_bytes() {
        let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mapping = mapping("tunnel.test", &target);
        let target_worker = thread::spawn(move || {
            let (mut stream, _) = target.accept().unwrap();
            let mut payload = [0; 4];
            stream.read_exact(&mut payload).unwrap();
            stream.write_all(b"pong").unwrap();
            payload
        });
        let service = WapProxyService::start(vec![mapping].into()).unwrap();
        let mut client = TcpStream::connect(service.endpoint()).unwrap();
        client
            .write_all(b"CONNECT tunnel.test:80 HTTP/1.1\r\n\r\nping")
            .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();

        assert_eq!(response, b"HTTP/1.1 200 Connection Established\r\n\r\npong");
        assert_eq!(target_worker.join().unwrap(), *b"ping");
    }

    #[test]
    fn origin_form_requests_use_the_host_header() {
        let ProxyRequest::Forward { authority, header } =
            parse_request(b"POST /submit HTTP/1.0\r\nHost: Example.Test:8080\r\nContent-Length: 0")
                .unwrap()
        else {
            panic!("origin-form request was not parsed as an HTTP forward");
        };

        assert_eq!(
            authority,
            Authority {
                host: "example.test".into(),
                port: 8080,
            }
        );
        assert_eq!(
            header,
            b"POST /submit HTTP/1.0\r\nHost: Example.Test:8080\r\nContent-Length: 0\r\n\r\n"
        );
    }
}
