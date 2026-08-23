use super::*;

pub(super) fn native_tostring(args: &[Value]) -> Value {
    let value = args.first().unwrap_or(&Value::Nil);
    value
        .bytes()
        .map(Value::Bytes)
        .unwrap_or_else(|| bytes(format!("{value:?}").as_bytes()))
}

pub(super) fn native_tonumber(args: &[Value]) -> Value {
    let Some(value) = args.first() else {
        return Value::Nil;
    };
    if let Value::Number(value) = value {
        return Value::Number(*value);
    }
    let Value::Bytes(value) = value else {
        return Value::Nil;
    };
    let Ok(text) = std::str::from_utf8(value) else {
        return Value::Nil;
    };
    let text = text.trim();
    let Some(base) = args.get(1) else {
        return text
            .parse::<f64>()
            .ok()
            .map(Value::Number)
            .unwrap_or(Value::Nil);
    };
    let Some(base) = base.number().map(|base| base as u32) else {
        return Value::Nil;
    };
    if !(2..=36).contains(&base) {
        return Value::Nil;
    }
    let (negative, digits) = text
        .strip_prefix('-')
        .map(|digits| (true, digits))
        .or_else(|| text.strip_prefix('+').map(|digits| (false, digits)))
        .unwrap_or((false, text));
    let Some(number) = i64::from_str_radix(digits, base).ok() else {
        return Value::Nil;
    };
    Value::Number(if negative { -number } else { number } as f64)
}

pub(super) fn integer_binary(
    args: &[Value],
    operation: impl FnOnce(i64, i64) -> i64,
) -> Result<Vec<Value>> {
    let left = integer_number(args.first().unwrap_or(&Value::Nil))?;
    let right = integer_number(args.get(1).unwrap_or(&Value::Nil))?;
    Ok(vec![Value::Number(operation(left, right) as f64)])
}

pub(super) fn string_byte(args: &[Value]) -> Result<Vec<Value>> {
    let string = value_bytes(args.first())?;
    let start = lua_index(args.get(1), string.len(), 1)?;
    let end = match args.get(2) {
        Some(value) => lua_index(Some(value), string.len(), string.len() as i64)?.saturating_add(1),
        None => start.saturating_add(1),
    };
    Ok((start..end.min(string.len()))
        .map(|index| Value::Number(f64::from(string[index])))
        .collect())
}

pub(super) fn string_char(args: &[Value]) -> Result<Vec<Value>> {
    let mut output = Vec::with_capacity(args.len());
    for value in args {
        output.push(integer_number(value)?.clamp(0, 255) as u8);
    }
    Ok(vec![Value::Bytes(output.into())])
}

pub(super) fn string_clen(args: &[Value]) -> Result<Vec<Value>> {
    let string = value_bytes(args.first())?;
    let len = string
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(string.len());
    Ok(vec![Value::Number(len as f64)])
}

pub(super) fn string_cstr(args: &[Value]) -> Result<Vec<Value>> {
    let string = value_bytes(args.first())?;
    let len = string
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(string.len());
    Ok(vec![Value::Bytes(Arc::from(&string[..len]))])
}

pub(super) fn string_sub(args: &[Value]) -> Result<Vec<Value>> {
    let string = value_bytes(args.first())?;
    let start = lua_index(args.get(1), string.len(), 1)?;
    let end_inclusive = lua_index(args.get(2), string.len(), string.len() as i64)?;
    let end = end_inclusive.saturating_add(1).min(string.len());
    Ok(vec![Value::Bytes(if start >= end {
        Arc::from(&b""[..])
    } else {
        Arc::from(&string[start..end])
    })])
}

pub(super) fn string_sub_value(args: &[Value]) -> Result<Vec<Value>> {
    let bits = match args.first().unwrap_or(&Value::Nil) {
        Value::Number(value) => value.to_bits(),
        Value::Bytes(value) => {
            let mut raw = [0_u8; 8];
            let len = value.len().min(raw.len());
            raw[..len].copy_from_slice(&value[..len]);
            u64::from_le_bytes(raw)
        }
        Value::Boolean(value) => u64::from(*value),
        Value::Nil => 0,
        other => {
            return Err(crate::Error::MrFault(format!(
                "string.subV cannot encode {other:?}"
            )));
        }
    };
    Ok(vec![
        Value::Number(f64::from(bits as u32)),
        Value::Number(f64::from((bits >> 32) as u32)),
    ])
}

pub(super) fn string_find(args: &[Value]) -> Result<Vec<Value>> {
    let haystack = value_bytes(args.first())?;
    let needle = value_bytes(args.get(1))?;
    let start = lua_index(args.get(2), haystack.len(), 1)?;
    let found = haystack[start..]
        .windows(needle.len())
        .position(|window| window == needle.as_ref());
    Ok(found
        .map(|offset| {
            let begin = start + offset;
            vec![
                Value::Number((begin + 1) as f64),
                Value::Number((begin + needle.len()) as f64),
            ]
        })
        .unwrap_or_else(|| vec![Value::Nil]))
}

pub(super) fn string_format(args: &[Value]) -> Result<Vec<Value>> {
    let format = value_bytes(args.first())?;
    let mut output = Vec::new();
    let mut values = args.iter().skip(1);
    let mut index = 0;
    while index < format.len() {
        if format[index] != b'%' || index + 1 >= format.len() {
            output.push(format[index]);
            index += 1;
            continue;
        }
        index += 1;
        if format[index] == b'%' {
            output.push(b'%');
            index += 1;
            continue;
        }
        while index < format.len() && b"-+ #0.123456789".contains(&format[index]) {
            index += 1;
        }
        let specifier = *format.get(index).unwrap_or(&b's');
        index += 1;
        let value = values.next().unwrap_or(&Value::Nil);
        match specifier {
            b'd' | b'i' | b'u' | b'x' | b'X' => {
                let integer = integer_number(value)?;
                let text = match specifier {
                    b'x' => format!("{integer:x}"),
                    b'X' => format!("{integer:X}"),
                    _ => integer.to_string(),
                };
                output.extend_from_slice(text.as_bytes());
            }
            b'f' | b'g' => output.extend_from_slice(number(value)?.to_string().as_bytes()),
            b'c' => output.push(integer_number(value)?.clamp(0, 255) as u8),
            _ => output.extend_from_slice(&value.bytes().unwrap_or_else(|| Arc::from(&b""[..]))),
        }
    }
    Ok(vec![Value::Bytes(output.into())])
}

pub(super) fn string_rep(args: &[Value]) -> Result<Vec<Value>> {
    let string = value_bytes(args.first())?;
    let count = usize::try_from(integer_number(args.get(1).unwrap_or(&Value::Nil))?).unwrap_or(0);
    let mut output = Vec::with_capacity(string.len().saturating_mul(count));
    for _ in 0..count {
        output.extend_from_slice(&string);
    }
    Ok(vec![Value::Bytes(output.into())])
}

pub(super) fn string_case(args: &[Value], upper: bool) -> Result<Vec<Value>> {
    let mut string = value_bytes(args.first())?.to_vec();
    for byte in &mut string {
        *byte = if upper {
            byte.to_ascii_uppercase()
        } else {
            byte.to_ascii_lowercase()
        };
    }
    Ok(vec![Value::Bytes(string.into())])
}

pub(super) fn string_pack(args: &[Value]) -> Result<Vec<Value>> {
    let format = value_bytes(args.first())?;
    let mut output = Vec::new();
    let mut values = args.iter().skip(1);
    let mut little_endian = true;
    for specifier in format.iter().copied() {
        match specifier {
            b'<' => little_endian = true,
            b'>' => little_endian = false,
            b' ' => {}
            b'i' | b'I' => {
                let value = integer_number(values.next().unwrap_or(&Value::Nil))? as u32;
                let encoded = if little_endian {
                    value.to_le_bytes()
                } else {
                    value.to_be_bytes()
                };
                output.extend_from_slice(&encoded);
            }
            other => {
                return Err(crate::Error::MrFault(format!(
                    "unsupported string.pack specifier {:?}",
                    char::from(other)
                )));
            }
        }
    }
    Ok(vec![Value::Bytes(output.into())])
}

pub(super) fn string_unpack(args: &[Value]) -> Result<Vec<Value>> {
    let format = value_bytes(args.first())?;
    let input = value_bytes(args.get(1))?;
    let mut offset = match args.get(2) {
        Some(value) => lua_index(Some(value), input.len(), 1)?,
        None => 0,
    };
    let mut output = Vec::new();
    let mut little_endian = true;
    for specifier in format.iter().copied() {
        match specifier {
            b'<' => little_endian = true,
            b'>' => little_endian = false,
            b' ' => {}
            b'i' | b'I' => {
                let end = offset
                    .checked_add(4)
                    .ok_or_else(|| crate::Error::MrFault("string.unpack offset overflow".into()))?;
                let raw: [u8; 4] = input
                    .get(offset..end)
                    .ok_or_else(|| {
                        crate::Error::MrFault(format!(
                            "string.unpack needs 4 bytes at offset {offset}, input has {}",
                            input.len()
                        ))
                    })?
                    .try_into()
                    .expect("checked four-byte field");
                let value = if little_endian {
                    u32::from_le_bytes(raw)
                } else {
                    u32::from_be_bytes(raw)
                };
                output.push(Value::Number(if specifier == b'i' {
                    f64::from(value as i32)
                } else {
                    f64::from(value)
                }));
                offset = end;
            }
            other => {
                return Err(crate::Error::MrFault(format!(
                    "unsupported string.unpack specifier {:?}",
                    char::from(other)
                )));
            }
        }
    }
    Ok(output)
}

pub(super) fn table_insert(args: &[Value]) -> Result<Vec<Value>> {
    let table = table(args.first())?;
    let (position, value) = if args.len() >= 3 {
        (integer_number(&args[1])?.max(1) as usize, args[2].clone())
    } else {
        (
            table.borrow().sequence_len() + 1,
            args.get(1).cloned().unwrap_or(Value::Nil),
        )
    };
    let length = table.borrow().sequence_len();
    for index in (position..=length).rev() {
        let current = table.borrow().get(&Value::Number(index as f64));
        table
            .borrow_mut()
            .set(Value::Number((index + 1) as f64), current);
    }
    table
        .borrow_mut()
        .set(Value::Number(position as f64), value);
    Ok(Vec::new())
}

pub(super) fn table_remove(args: &[Value]) -> Result<Vec<Value>> {
    let table = table(args.first())?;
    let position = args
        .get(1)
        .map(integer_number)
        .transpose()?
        .map(|value| value.max(1) as usize)
        .unwrap_or_else(|| table.borrow().sequence_len());
    let value = table.borrow_mut().remove_sequence(position);
    Ok(vec![value])
}

pub(super) fn table_concat(args: &[Value]) -> Result<Vec<Value>> {
    let table = table(args.first())?;
    let separator = args
        .get(1)
        .and_then(Value::bytes)
        .unwrap_or_else(|| Arc::from(&b""[..]));
    let length = table.borrow().sequence_len();
    let mut output = Vec::new();
    for index in 1..=length {
        if index > 1 {
            output.extend_from_slice(&separator);
        }
        let value = table.borrow().get(&Value::Number(index as f64));
        output.extend_from_slice(&value.bytes().unwrap_or_else(|| Arc::from(&b""[..])));
    }
    Ok(vec![Value::Bytes(output.into())])
}

pub(super) fn lua_index(value: Option<&Value>, len: usize, default: i64) -> Result<usize> {
    let index = value.map(integer_number).transpose()?.unwrap_or(default);
    let zero_based = if index > 0 {
        index - 1
    } else if index < 0 {
        len as i64 + index
    } else {
        0
    };
    Ok(zero_based.clamp(0, len as i64) as usize)
}

pub(super) fn safe_work_path(work_dir: &std::path::Path, bytes: &[u8]) -> Option<PathBuf> {
    let mut path = std::str::from_utf8(bytes).ok()?;
    if path.starts_with('/') || path.starts_with('\\') {
        return None;
    }
    let mut resolved = work_dir.to_path_buf();
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        match path.as_bytes()[0].to_ascii_uppercase() {
            b'C' => {}
            drive @ (b'X' | b'Y' | b'Z') => {
                resolved.push("disk");
                resolved.push(char::from(drive.to_ascii_lowercase()).to_string());
            }
            _ => return None,
        }
        path = &path[2..];
        if !path.is_empty() && !path.starts_with('/') && !path.starts_with('\\') {
            return None;
        }
    }
    for component in path
        .split(['/', '\\'])
        .filter(|component| !matches!(*component, "" | "."))
    {
        if component == ".." || component.contains('\0') || component.contains(':') {
            return None;
        }
        resolved.push(component);
    }
    Some(resolved)
}
