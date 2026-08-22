use super::*;

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
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip.to_be_bytes()), port));
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
        let read = stream.read(&mut bytes).ok()?;
        bytes.truncate(read);
        Some(bytes)
    }

    pub(super) fn send_native_socket(&mut self, handle: i32, bytes: &[u8]) -> Option<usize> {
        if bytes.len() > self.heap_len || self.native_socket_state(handle) != 0 {
            return None;
        }
        let socket = self.native_sockets.get_mut(&handle)?;
        let NativeSocketState::Connected(stream) = &mut socket.state else {
            return None;
        };
        stream.write(bytes).ok()
    }
}
