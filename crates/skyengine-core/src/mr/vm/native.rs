use super::stdlib::*;
use super::*;
use std::cell::RefCell;

impl MrVm {
    pub(super) fn native(&self, name: &'static str) {
        self.globals
            .borrow_mut()
            .set(bytes(name.as_bytes()), Value::Native(name));
    }

    pub(super) fn register_libraries(&mut self) {
        for name in [
            "type",
            "tostring",
            "tonumber",
            "print",
            "next",
            "pairs",
            "ipairs",
            "assert",
            "error",
            "pcall",
            "_pCall",
            "_loads",
            "dofile",
            "collectgarbage",
            "_gc",
            "GetSysInfo",
            "_platEx",
            "BitmapLoad",
            "BitmapShow",
            "SpriteSet",
            "SpriteDraw",
            "DrawRect",
            "_drawRect",
            "_effSetCon",
            "DrawLine",
            "_drawLine",
            "DrawText",
            "DispUpEx",
            "TestCom",
            "TestCom1",
            "_com",
            "_closeNet",
            "_strCom",
            "LoadTable",
            "SaveTable",
            "LoadPack",
            "RunFile",
            "UAReset",
            "Exit",
            "TimerStart",
            "TimerStop",
            "_num",
            "_str",
            "_t",
            "mod",
            "_mod",
            "_and",
            "_or",
            "_xor",
            "_not",
            "clen",
            "cstr",
            "_textWidth",
        ] {
            self.native(name);
        }

        let string = Table::new();
        for name in [
            "byte", "char", "len", "clen", "cstr", "sub", "subV", "find", "format", "rep", "lower",
            "upper", "pack", "unpack", "new", "update",
        ] {
            string
                .borrow_mut()
                .set(bytes(name.as_bytes()), Value::Native(name));
        }
        self.globals
            .borrow_mut()
            .set(bytes(b"string"), Value::Table(string));

        let table = Table::new();
        for name in ["insert", "remove", "getn", "concat"] {
            table
                .borrow_mut()
                .set(bytes(name.as_bytes()), Value::Native(name));
        }
        self.globals
            .borrow_mut()
            .set(bytes(b"table"), Value::Table(table));

        let file = Table::new();
        for (name, native) in [
            ("exist", "exist"),
            ("open", "file_open"),
            ("close", "file_close"),
            ("remove", "file_remove"),
            ("rename", "rename"),
            ("getlen", "getlen"),
        ] {
            file.borrow_mut()
                .set(bytes(name.as_bytes()), Value::Native(native));
        }
        self.globals
            .borrow_mut()
            .set(bytes(b"file"), Value::Table(file));

        let sys = Table::new();
        for (name, native) in [
            ("rm", "file_remove"),
            ("getInfo", "sys_get_info"),
            ("findstart", "sys_find_start"),
            ("findnext", "sys_find_next"),
            ("findstop", "sys_find_stop"),
            ("findStart", "sys_find_start"),
            ("findNext", "sys_find_next"),
            ("findStop", "sys_find_stop"),
        ] {
            sys.borrow_mut()
                .set(bytes(name.as_bytes()), Value::Native(native));
        }
        self.globals
            .borrow_mut()
            .set(bytes(b"sys"), Value::Table(sys));
        self.globals
            .borrow_mut()
            .set(bytes(b"socket"), Value::Table(self.host.socket_library()));
        self.globals
            .borrow_mut()
            .set(bytes(b"SCROLL_W"), Value::Number(0.0));
    }

    pub(super) fn call_native(&mut self, name: &'static str, args: &[Value]) -> Result<Vec<Value>> {
        let trace = std::env::var_os("SKYENGINE_TRACE_MR_CALLS").is_some();
        if trace {
            eprintln!("[mr-call] {name}({})", trace_values(args));
        }
        let result = match name {
            "type" => Ok(vec![bytes(args.first().unwrap_or(&Value::Nil).type_name())]),
            "tostring" | "_str" => Ok(vec![native_tostring(args)]),
            "tonumber" | "_num" => Ok(vec![native_tonumber(args)]),
            "_t" => Ok(vec![bytes(match args.first().unwrap_or(&Value::Nil) {
                Value::Nil => b"nil",
                Value::Boolean(_) => b"bool",
                Value::Number(_) => b"num",
                Value::Bytes(_) | Value::Buffer(_) => b"str",
                Value::Table(_) => b"tab",
                Value::Closure(_) | Value::Native(_) => b"fun",
            })]),
            "print" => {
                let message = args
                    .iter()
                    .map(|value| {
                        value
                            .bytes()
                            .map(|bytes| {
                                let len = bytes
                                    .iter()
                                    .position(|byte| *byte == 0)
                                    .unwrap_or(bytes.len());
                                String::from_utf8_lossy(&bytes[..len]).into_owned()
                            })
                            .unwrap_or_else(|| format!("{value:?}"))
                    })
                    .collect::<Vec<_>>()
                    .join("\t");
                eprintln!("[mr] {message}");
                Ok(Vec::new())
            }
            "next" => {
                let table = table(args.first())?;
                let previous = args.get(1).unwrap_or(&Value::Nil);
                Ok(table
                    .borrow()
                    .next(previous)
                    .map(|(key, value)| vec![key, value])
                    .unwrap_or_default())
            }
            "pairs" | "ipairs" => Ok(vec![
                Value::Native("next"),
                args.first().cloned().unwrap_or(Value::Nil),
                Value::Nil,
            ]),
            "assert" => {
                if args.first().is_some_and(Value::truthy) {
                    Ok(args.to_vec())
                } else {
                    Err(crate::Error::MrFault(
                        args.get(1)
                            .and_then(Value::bytes)
                            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                            .unwrap_or_else(|| "assertion failed".into()),
                    ))
                }
            }
            "error" => Err(crate::Error::MrFault(
                args.first()
                    .and_then(Value::bytes)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_else(|| "error".into()),
            )),
            "collectgarbage" | "_gc" => Ok(Vec::new()),
            "TestCom1" if matches!(args.first(), Some(Value::Number(command)) if *command == 300.0) =>
            {
                let source = args
                    .get(1)
                    .and_then(Value::bytes)
                    .ok_or_else(|| crate::Error::MrFault("TestCom1(300) expects source".into()))?;
                Ok(vec![Value::Closure(Rc::new(Closure {
                    prototype: text::compile(&source, &self.limits)?,
                    upvalues: Vec::new(),
                }))])
            }
            "_loads" => match args.first() {
                Some(Value::Closure(closure)) => Ok(vec![Value::Closure(closure.clone())]),
                _ => Ok(vec![Value::Nil]),
            },
            "mod" | "_mod" => integer_binary(
                args,
                |left, right| {
                    if right == 0 { 0 } else { left % right }
                },
            ),
            "_and" => integer_binary(args, |left, right| left & right),
            "_or" => integer_binary(args, |left, right| left | right),
            "_xor" => integer_binary(args, |left, right| left ^ right),
            "_not" => Ok(vec![Value::Number(
                (!integer_number(args.first().unwrap_or(&Value::Nil))?) as f64,
            )]),
            "byte" => string_byte(args),
            "char" => string_char(args),
            "len" => Ok(vec![Value::Number(value_bytes(args.first())?.len() as f64)]),
            "clen" => string_clen(args),
            "cstr" => string_cstr(args),
            "sub" => string_sub(args),
            "subV" => string_sub_value(args),
            "find" => string_find(args),
            "format" => string_format(args),
            "rep" => string_rep(args),
            "lower" => string_case(args, false),
            "upper" => string_case(args, true),
            "pack" => string_pack(args),
            "unpack" => string_unpack(args),
            "new" => {
                let len = usize::try_from(integer_number(args.first().unwrap_or(&Value::Nil))?)
                    .map_err(|_| crate::Error::MrFault("string.new length is negative".into()))?;
                if len > self.limits.max_mr_string_len {
                    return Err(crate::Error::ResourceLimit(format!(
                        "string.new length {len} exceeds {}",
                        self.limits.max_mr_string_len
                    )));
                }
                Ok(vec![Value::Buffer(Rc::new(RefCell::new(vec![0; len])))])
            }
            "update" => string_update(args),
            "insert" => table_insert(args),
            "remove" => table_remove(args),
            "getn" => Ok(vec![Value::Number(
                table(args.first())?.borrow().sequence_len() as f64,
            )]),
            "concat" => table_concat(args),
            "exist" => self.file_exist(args),
            "file_open" => self.file_open(args),
            "file_read" => self.file_read(args),
            "file_seek" => self.file_seek(args),
            "file_write" => self.file_write(args),
            "file_close" => self.file_close(args),
            "file_remove" => self.file_remove(args),
            "LoadPack" => {
                let name = args.first().and_then(Value::bytes);
                if self.host.load_pack(name.as_deref())? && name.is_some() {
                    self.set_global(b"loadfile", Value::Native("load_pack_file"))?;
                    Ok(vec![Value::Native("loaded_pack")])
                } else {
                    self.set_global(b"loadfile", Value::Nil)?;
                    Ok(vec![Value::Nil])
                }
            }
            "load_pack_file" => {
                let name = value_bytes(args.first())?;
                let source = self.host.read_loaded_pack(&name)?;
                let prototype = if source.starts_with(SIGNATURE) {
                    MrChunk::load(&source, &self.limits)?.root
                } else {
                    text::compile(&source, &self.limits)?
                };
                Ok(vec![Value::Closure(Rc::new(Closure {
                    prototype,
                    upvalues: Vec::new(),
                }))])
            }
            "rename" | "getlen" => Ok(vec![Value::Number(-1.0)]),
            _ => self.host.call(name, args),
        };
        if result.is_ok()
            && name == "_strCom"
            && matches!(args.first(), Some(Value::Number(command)) if *command == 800.0)
        {
            self.set_global(b"_mr_c_load", Value::Native("mr_c_load"))?;
        }
        if trace {
            match &result {
                Ok(values) => eprintln!("[mr-return] {name} -> {}", trace_values(values)),
                Err(error) => eprintln!("[mr-return] {name} -> error: {error}"),
            }
        }
        result
    }

    pub(super) fn file_exist(&self, args: &[Value]) -> Result<Vec<Value>> {
        let name = value_bytes(args.first())?;
        let in_package = self
            .host
            .package
            .entries()
            .iter()
            .any(|entry| entry.name == name.as_ref());
        let external = safe_work_path(&self.host.work_dir, &name).is_some_and(|path| path.exists());
        Ok(vec![Value::Number(if in_package || external {
            1.0
        } else {
            0.0
        })])
    }

    fn file_open(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let name = value_bytes(args.first())?;
        let mode = u32::try_from(integer_number(args.get(1).unwrap_or(&Value::Nil))?)
            .map_err(|_| crate::Error::MrFault("file.open mode is negative".into()))?;
        let handle = self.host.mr_file_open(&name, mode)?;
        if handle < 0 {
            return Ok(vec![Value::Nil]);
        }
        let file = Table::new();
        let mut values = file.borrow_mut();
        values.set(bytes(b"__handle"), Value::Number(f64::from(handle)));
        values.set(bytes(b"read"), Value::Native("file_read"));
        values.set(bytes(b"seek"), Value::Native("file_seek"));
        values.set(bytes(b"write"), Value::Native("file_write"));
        values.set(bytes(b"close"), Value::Native("file_close"));
        drop(values);
        Ok(vec![Value::Table(file)])
    }

    fn file_write(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = file_handle(args.first())?;
        let data = value_bytes(args.get(1))?;
        Ok(vec![Value::Number(
            self.host
                .mr_file_write(handle, &data)?
                .and_then(|len| u32::try_from(len).ok())
                .map(f64::from)
                .unwrap_or(-1.0),
        )])
    }

    fn file_read(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = file_handle(args.first())?;
        let len = usize::try_from(integer_number(args.get(1).unwrap_or(&Value::Nil))?)
            .map_err(|_| crate::Error::MrFault("file.read length is negative".into()))?;
        if len > self.limits.max_mr_string_len {
            return Err(crate::Error::ResourceLimit(format!(
                "file.read length {len} exceeds {}",
                self.limits.max_mr_string_len
            )));
        }
        Ok(vec![
            self.host
                .mr_file_read(handle, len)?
                .map_or(Value::Nil, |data| Value::Bytes(data.into())),
        ])
    }

    fn file_seek(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = file_handle(args.first())?;
        let offset = i32::try_from(integer_number(args.get(1).unwrap_or(&Value::Nil))?)
            .map_err(|_| crate::Error::MrFault("file.seek offset is out of range".into()))?;
        let origin = u32::try_from(integer_number(args.get(2).unwrap_or(&Value::Nil))?)
            .map_err(|_| crate::Error::MrFault("file.seek origin is negative".into()))?;
        Ok(vec![
            self.host
                .mr_file_seek(handle, offset, origin)?
                .and_then(|position| u32::try_from(position).ok())
                .map(|position| Value::Number(f64::from(position)))
                .unwrap_or(Value::Nil),
        ])
    }

    fn file_close(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = file_handle(args.first())?;
        Ok(vec![Value::Number(f64::from(
            self.host.mr_file_close(handle)?,
        ))])
    }

    fn file_remove(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let name = value_bytes(args.first())?;
        Ok(vec![Value::Number(f64::from(
            self.host.mr_file_remove(&name)?,
        ))])
    }
}

fn file_handle(value: Option<&Value>) -> Result<i32> {
    let handle = match value {
        Some(Value::Table(file)) => integer_number(&file.borrow().get(&bytes(b"__handle"))),
        Some(value) => integer_number(value),
        None => Err(crate::Error::MrFault("file handle is missing".into())),
    }?;
    i32::try_from(handle).map_err(|_| crate::Error::MrFault("file handle is out of range".into()))
}
