use super::heap::aligned_heap_len;
use super::*;

impl ExtRuntime {
    pub(super) fn dispatch_libc(&mut self, slot: u32, cpu: &mut ArmCpu) -> Result<()> {
        match slot {
            0 => {
                let address = self.allocate_guest_block(cpu.register(0) as usize)?;
                cpu.set_register(0, address.map_or(0, |address| address.0));
            }
            1 => {
                self.free_guest_block(GuestAddr(cpu.register(0)), cpu.register(1) as usize)?;
                cpu.set_register(0, 0);
            }
            2 => {
                let source = GuestAddr(cpu.register(0));
                let old_len = cpu.register(1) as usize;
                let new_len = cpu.register(2) as usize;
                if source.0 == 0 {
                    let output = self.allocate_guest_block(new_len)?;
                    cpu.set_register(0, output.map_or(0, |address| address.0));
                } else if new_len == 0 {
                    self.free_guest_block(source, old_len)?;
                    cpu.set_register(0, 0);
                } else if new_len <= old_len {
                    let old_aligned = aligned_heap_len(old_len)?;
                    let new_aligned = aligned_heap_len(new_len)?;
                    if old_aligned - new_aligned >= FREE_BLOCK_HEADER_LEN {
                        self.free_guest_block(
                            GuestAddr(source.0.wrapping_add(new_aligned)),
                            (old_aligned - new_aligned) as usize,
                        )?;
                    }
                    cpu.set_register(0, source.0);
                } else {
                    let Some(output) = self.allocate_guest_block(new_len)? else {
                        cpu.set_register(0, 0);
                        return Ok(());
                    };
                    let bytes = self.memory.read(source, old_len)?;
                    self.memory.write(output, &bytes)?;
                    self.free_guest_block(source, old_len)?;
                    cpu.set_register(0, output.0);
                }
            }
            3 | 4 => {
                let destination = GuestAddr(cpu.register(0));
                let bytes = self
                    .memory
                    .read(GuestAddr(cpu.register(1)), cpu.register(2) as usize)?;
                self.memory.write(destination, &bytes)?;
                cpu.set_register(0, destination.0);
            }
            5 => {
                let destination = GuestAddr(cpu.register(0));
                let bytes = self.read_c_string(GuestAddr(cpu.register(1)), 1024 * 1024)?;
                self.memory.write(destination, &bytes)?;
                self.memory
                    .write_u8(destination.checked_add(bytes.len() as u32)?, 0)?;
                cpu.set_register(0, destination.0);
            }
            6 => {
                let destination = GuestAddr(cpu.register(0));
                let len = cpu.register(2) as usize;
                let source = self.read_c_string_bounded(GuestAddr(cpu.register(1)), len)?;
                let mut bytes = vec![0; len];
                let copied = source.len().min(len);
                bytes[..copied].copy_from_slice(&source[..copied]);
                self.memory.write(destination, &bytes)?;
                cpu.set_register(0, destination.0);
            }
            7 | 8 => {
                let destination = GuestAddr(cpu.register(0));
                let destination_len = self.read_c_string(destination, 1024 * 1024)?.len();
                let source = if slot == 8 {
                    self.read_c_string_bounded(
                        GuestAddr(cpu.register(1)),
                        cpu.register(2) as usize,
                    )?
                } else {
                    self.read_c_string(GuestAddr(cpu.register(1)), 1024 * 1024)?
                };
                let append_at = destination.checked_add(destination_len as u32)?;
                self.memory.write(append_at, &source)?;
                self.memory
                    .write_u8(append_at.checked_add(source.len() as u32)?, 0)?;
                cpu.set_register(0, destination.0);
            }
            9 => {
                let len = cpu.register(2) as usize;
                let left = self.memory.read(GuestAddr(cpu.register(0)), len)?;
                let right = self.memory.read(GuestAddr(cpu.register(1)), len)?;
                cpu.set_register(0, compare_bytes(&left, &right) as u32);
            }
            10 | 12 => {
                let left = self.read_c_string(GuestAddr(cpu.register(0)), 1024 * 1024)?;
                let right = self.read_c_string(GuestAddr(cpu.register(1)), 1024 * 1024)?;
                cpu.set_register(0, compare_bytes(&left, &right) as u32);
            }
            11 => {
                let limit = cpu.register(2) as usize;
                let left = self.read_c_string_bounded(GuestAddr(cpu.register(0)), limit)?;
                let right = self.read_c_string_bounded(GuestAddr(cpu.register(1)), limit)?;
                cpu.set_register(0, compare_bytes(&left, &right) as u32);
            }
            13 => {
                let start = cpu.register(0);
                let needle = cpu.register(1) as u8;
                let bytes = self
                    .memory
                    .read(GuestAddr(start), cpu.register(2) as usize)?;
                cpu.set_register(
                    0,
                    bytes
                        .iter()
                        .position(|byte| *byte == needle)
                        .map(|offset| start + offset as u32)
                        .unwrap_or(0),
                );
            }
            14 => {
                let destination = GuestAddr(cpu.register(0));
                let value = cpu.register(1) as u8;
                let len = cpu.register(2) as usize;
                self.memory.write(destination, &vec![value; len])?;
                cpu.set_register(0, destination.0);
            }
            15 => {
                let len = self
                    .read_c_string(GuestAddr(cpu.register(0)), 1024 * 1024)?
                    .len();
                cpu.set_register(0, len as u32);
            }
            16 => {
                let start = cpu.register(0);
                let haystack = self.read_c_string(GuestAddr(start), 1024 * 1024)?;
                let needle = self.read_c_string(GuestAddr(cpu.register(1)), 1024 * 1024)?;
                let found = if needle.is_empty() {
                    Some(0)
                } else {
                    haystack
                        .windows(needle.len())
                        .position(|window| window == needle)
                };
                cpu.set_register(0, found.map(|offset| start + offset as u32).unwrap_or(0));
            }
            17 => self.sprintf(cpu)?,
            18 => {
                let text = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, parse_integer(&text, 10).0 as u32);
            }
            19 => {
                let source = cpu.register(0);
                let text = self.read_c_string(GuestAddr(source), 1024)?;
                let base = cpu.register(2);
                let (value, consumed) = parse_integer(&text, base);
                let end_pointer = GuestAddr(cpu.register(1));
                if end_pointer.0 != 0 {
                    self.memory
                        .write_u32(end_pointer, source.wrapping_add(consumed as u32))?;
                }
                cpu.set_register(0, value as u32);
            }
            20 => {
                self.random_state = self
                    .random_state
                    .wrapping_mul(1_103_515_245)
                    .wrapping_add(12_345);
                cpu.set_register(0, (self.random_state >> 16) & 0x7fff);
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    pub(super) fn sprintf(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let destination = GuestAddr(cpu.register(0));
        let format = self.read_c_string(GuestAddr(cpu.register(1)), 64 * 1024)?;
        let stack_pointer = cpu.register(13);
        let mut argument_index = 0_u32;
        let mut next_argument = |memory: &GuestMemory| -> Result<u32> {
            let value = match argument_index {
                0 => cpu.register(2),
                1 => cpu.register(3),
                index => memory.read_u32(GuestAddr(stack_pointer + (index - 2) * 4))?,
            };
            argument_index += 1;
            Ok(value)
        };
        let mut output = Vec::new();
        let mut index = 0;
        while index < format.len() {
            if format[index] != b'%' {
                output.push(format[index]);
                index += 1;
                continue;
            }
            index += 1;
            if format.get(index) == Some(&b'%') {
                output.push(b'%');
                index += 1;
                continue;
            }
            while format
                .get(index)
                .is_some_and(|byte| b"-+ #0.123456789hl".contains(byte))
            {
                index += 1;
            }
            let specifier = *format
                .get(index)
                .ok_or_else(|| Error::Abi("sprintf format ends after '%'".into()))?;
            index += 1;
            let argument = next_argument(&self.memory)?;
            match specifier {
                b's' => {
                    output.extend_from_slice(&self.read_c_string(GuestAddr(argument), 1024 * 1024)?)
                }
                b'c' => output.push(argument as u8),
                b'd' | b'i' => output.extend_from_slice((argument as i32).to_string().as_bytes()),
                b'u' => output.extend_from_slice(argument.to_string().as_bytes()),
                b'x' => output.extend_from_slice(format!("{argument:x}").as_bytes()),
                b'X' => output.extend_from_slice(format!("{argument:X}").as_bytes()),
                b'p' => output.extend_from_slice(format!("0x{argument:08x}").as_bytes()),
                other => {
                    return Err(Error::Abi(format!(
                        "unsupported sprintf specifier {:?}",
                        char::from(other)
                    )));
                }
            }
        }
        self.memory.write(destination, &output)?;
        self.memory
            .write_u8(destination.checked_add(output.len() as u32)?, 0)?;
        cpu.set_register(0, output.len() as u32);
        Ok(())
    }
}

fn compare_bytes(left: &[u8], right: &[u8]) -> i32 {
    for (left, right) in left.iter().copied().zip(right.iter().copied()) {
        if left != right {
            return i32::from(left) - i32::from(right);
        }
    }
    left.len().cmp(&right.len()) as i32
}

fn parse_integer(input: &[u8], requested_base: u32) -> (i64, usize) {
    let mut index = 0;
    while input.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let negative = input.get(index) == Some(&b'-');
    if negative || input.get(index) == Some(&b'+') {
        index += 1;
    }
    let mut base = requested_base;
    if base == 0 {
        base = if input
            .get(index..index + 2)
            .is_some_and(|prefix| prefix[0] == b'0' && matches!(prefix[1], b'x' | b'X'))
        {
            16
        } else if input.get(index) == Some(&b'0') {
            8
        } else {
            10
        };
    }
    if base == 16
        && input
            .get(index..index + 2)
            .is_some_and(|prefix| prefix[0] == b'0' && matches!(prefix[1], b'x' | b'X'))
    {
        index += 2;
    }
    if !(2..=36).contains(&base) {
        return (0, index);
    }
    let digit_start = index;
    let mut value = 0_i64;
    while let Some(digit) = input.get(index).and_then(|byte| match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'Z' => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }) {
        if digit >= base {
            break;
        }
        value = value
            .saturating_mul(i64::from(base))
            .saturating_add(i64::from(digit));
        index += 1;
    }
    if index == digit_start {
        return (0, digit_start);
    }
    (if negative { -value } else { value }, index)
}
