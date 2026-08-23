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
        let endpoint = self.route_mr_endpoint(ip, port);
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

        let address = SocketAddr::V4(endpoint);
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
        let Some(MrSocket {
            state: MrSocketState::Connected(stream),
        }) = self.mr_sockets.get_mut(&handle)
        else {
            return Ok(vec![Value::Number(-1.0)]);
        };
        let sent = match stream.write(&input) {
            Ok(sent) => sent as f64,
            Err(error) if error.kind() == ErrorKind::WouldBlock => 0.0,
            Err(_) => {
                self.mr_sockets.get_mut(&handle).unwrap().state = MrSocketState::Failed;
                -1.0
            }
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

    fn route_mr_endpoint(&self, ip: Ipv4Addr, port: u16) -> SocketAddrV4 {
        if let Some(mapping) = self.dns_mappings.iter().find(|mapping| {
            mapping
                .source
                .parse::<Ipv4Addr>()
                .is_ok_and(|source| source == ip)
        }) {
            return SocketAddrV4::new(mapping.address, mapping.port.unwrap_or(port));
        }
        if let Some(mapping) = self
            .dns_mappings
            .iter()
            .find(|mapping| mapping.port.is_some() && mapping.address == ip)
        {
            return SocketAddrV4::new(ip, mapping.port.unwrap());
        }
        SocketAddrV4::new(ip, port)
    }
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
