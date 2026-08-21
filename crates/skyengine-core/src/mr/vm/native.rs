use super::stdlib::*;
use super::*;

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
            "DrawLine",
            "_drawLine",
            "DrawText",
            "DispUpEx",
            "TestCom",
            "_com",
            "_strCom",
            "LoadTable",
            "SaveTable",
            "LoadPack",
            "UAReset",
            "Exit",
            "TimerStart",
            "TimerStop",
            "_num",
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
            "upper", "pack", "unpack",
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
            ("open", "open"),
            ("close", "close"),
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
            .set(bytes(b"SCROLL_W"), Value::Number(0.0));
    }

    pub(super) fn call_native(&mut self, name: &'static str, args: &[Value]) -> Result<Vec<Value>> {
        let trace = std::env::var_os("SKYENGINE_TRACE_MR_CALLS").is_some();
        if trace {
            eprintln!("[mr-call] {name}({})", trace_values(args));
        }
        let result = match name {
            "type" => Ok(vec![bytes(args.first().unwrap_or(&Value::Nil).type_name())]),
            "tostring" => Ok(vec![
                args.first()
                    .unwrap_or(&Value::Nil)
                    .bytes()
                    .map(Value::Bytes)
                    .unwrap_or_else(|| bytes(format!("{:?}", args[0]).as_bytes())),
            ]),
            "tonumber" | "_num" => Ok(vec![native_tonumber(args)]),
            "_t" => Ok(vec![bytes(match args.first().unwrap_or(&Value::Nil) {
                Value::Nil => b"nil",
                Value::Boolean(_) => b"bool",
                Value::Number(_) => b"num",
                Value::Bytes(_) => b"str",
                Value::Table(_) => b"tab",
                Value::Closure(_) | Value::Native(_) => b"fun",
            })]),
            "print" => {
                let message = args
                    .iter()
                    .map(|value| {
                        value
                            .bytes()
                            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
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
            "insert" => table_insert(args),
            "remove" => table_remove(args),
            "getn" => Ok(vec![Value::Number(
                table(args.first())?.borrow().sequence_len() as f64,
            )]),
            "concat" => table_concat(args),
            "exist" => self.file_exist(args),
            "open" => Ok(vec![Value::Nil]),
            "close" => Ok(vec![Value::Number(0.0)]),
            "rename" | "file_remove" | "getlen" => Ok(vec![Value::Number(-1.0)]),
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
}
