use std::{cmp::Ordering, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use crate::{
    Error, Framebuffer, Package, PlatformDisplay, ResourceLimits, Result, arm::ExtLifecycleRequest,
};

use super::{
    chunk::{Constant, MrChunk, MrProfile, Prototype},
    host::{MrHost, MrHostConfig, PreparedEntry},
    value::{Cell, Closure, ClosureRef, Table, TableRef, Value},
};

mod native;
mod stdlib;

const RK_LIMIT: usize = 250;
const SIGNATURE: &[u8; 4] = b"\x1bMRP";
const EXT_SIGNATURE: &[u8; 8] = b"MRPGCMAP";

pub struct MrVm {
    globals: TableRef,
    frames: Vec<Frame>,
    host: MrHost,
    limits: ResourceLimits,
    instruction_count: u64,
    final_values: Vec<Value>,
    native_entry: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LifecycleOutcome {
    Continue,
    ExitRequested,
}

#[derive(Debug)]
pub(crate) enum LifecycleError {
    BeforeCommit(Error),
    AfterCommit(Error),
}

struct Frame {
    closure: ClosureRef,
    registers: Vec<Cell>,
    pc: usize,
    top: usize,
    return_target: Option<ReturnTarget>,
}

#[derive(Clone, Copy)]
struct ReturnTarget {
    register: usize,
    expected: Option<usize>,
    protected: bool,
}

enum CallResult {
    Immediate(Vec<Value>),
    Pushed,
}

impl MrVm {
    pub(crate) fn new(
        package: Arc<Package>,
        framebuffer: Framebuffer,
        display: Box<dyn PlatformDisplay>,
        host_config: MrHostConfig,
        limits: ResourceLimits,
    ) -> Self {
        let globals = Table::new();
        let mut vm = Self {
            globals,
            frames: Vec::new(),
            host: MrHost::new(package, framebuffer, display, host_config),
            limits,
            instruction_count: 0,
            final_values: Vec::new(),
            native_entry: false,
        };
        vm.register_libraries();
        vm
    }

    pub fn framebuffer(&self) -> &Framebuffer {
        &self.host.framebuffer
    }

    pub fn display_mut(&mut self) -> &mut dyn PlatformDisplay {
        self.host.display.as_mut()
    }

    pub fn native_timer_due_in(&self) -> Option<Duration> {
        self.host.native_timer_due_in()
    }

    pub fn dispatch_native_timer(&mut self) -> Result<bool> {
        if !self.host.take_due_native_timer()? {
            return Ok(false);
        }
        if self.native_entry || !self.call_global(b"dealtimer", Vec::new())? {
            self.host.dispatch_native_timer()?;
        }
        Ok(true)
    }

    pub fn dispatch_external_action_completion(&mut self) -> Result<bool> {
        self.host.dispatch_external_action_completion()
    }

    pub fn dispatch_pending_platform_event(&mut self) -> Result<bool> {
        self.host.dispatch_pending_platform_event()
    }

    pub fn run_entry(&mut self, entry: &[u8]) -> Result<()> {
        self.host.set_current_entry(entry);
        let bytes = self.host.package.read_named(entry)?;
        if bytes.starts_with(EXT_SIGNATURE) {
            self.native_entry = true;
            return self.host.run_native_entry(&bytes);
        }
        self.native_entry = false;
        if !bytes.starts_with(SIGNATURE) {
            return Err(crate::Error::UnsupportedMr(format!(
                "text MR frontend is not implemented for {}",
                String::from_utf8_lossy(entry)
            )));
        }
        let chunk = MrChunk::load(&bytes, &self.limits)?;
        if std::env::var_os("SKYENGINE_TRACE_MR_PROTOTYPES").is_some() {
            trace_prototype(&chunk.root, 0);
        }
        let closure = Rc::new(Closure {
            prototype: chunk.root,
            upvalues: Vec::new(),
        });
        self.push_frame(closure, Vec::new(), None)?;
        self.run_frames()
    }

    pub fn call_global(&mut self, name: &[u8], args: Vec<Value>) -> Result<bool> {
        let function = self.global(name);
        if matches!(function, Value::Nil) {
            return Ok(false);
        }
        match self.call_value(function, args, None, false)? {
            CallResult::Immediate(values) => self.final_values = values,
            CallResult::Pushed => self.run_frames()?,
        }
        Ok(true)
    }

    pub(crate) fn process_lifecycle_request(
        &mut self,
    ) -> std::result::Result<LifecycleOutcome, LifecycleError> {
        let Some(request) = self
            .host
            .lifecycle_request()
            .map_err(LifecycleError::BeforeCommit)?
        else {
            return Ok(LifecycleOutcome::Continue);
        };
        match request {
            ExtLifecycleRequest::Restart { package, entry } => {
                let prepared = match self.host.prepare_restart(&package, &entry, &self.limits) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        self.host
                            .acknowledge_lifecycle_request()
                            .map_err(LifecycleError::BeforeCommit)?;
                        return Err(LifecycleError::BeforeCommit(error));
                    }
                };
                self.host
                    .acknowledge_lifecycle_request()
                    .map_err(LifecycleError::BeforeCommit)?;
                let prepared_entry = self.host.commit_application(prepared);
                self.globals = Table::new();
                self.frames.clear();
                self.instruction_count = 0;
                self.final_values.clear();
                self.native_entry = false;
                self.register_libraries();
                if let Err(error) = self.run_prepared_entry(prepared_entry) {
                    self.host.discard_failed_application_runtime();
                    self.frames.clear();
                    self.final_values.clear();
                    self.native_entry = false;
                    return Err(LifecycleError::AfterCommit(error));
                }
                Ok(LifecycleOutcome::Continue)
            }
            ExtLifecycleRequest::Exit => Ok(LifecycleOutcome::ExitRequested),
        }
    }

    fn run_prepared_entry(&mut self, prepared: PreparedEntry) -> Result<()> {
        match prepared {
            PreparedEntry::Native(bytes) => {
                self.native_entry = true;
                self.host.run_native_entry(&bytes)
            }
            PreparedEntry::Mr(prototype) => {
                self.native_entry = false;
                if std::env::var_os("SKYENGINE_TRACE_MR_PROTOTYPES").is_some() {
                    trace_prototype(&prototype, 0);
                }
                let closure = Rc::new(Closure {
                    prototype,
                    upvalues: Vec::new(),
                });
                self.push_frame(closure, Vec::new(), None)?;
                self.run_frames()
            }
        }
    }

    pub fn route_key_event(&mut self, code: i32, pressed: bool) -> Result<Option<(i32, i32, i32)>> {
        let event = self.host.route_key_event(code, pressed)?;
        if self.native_entry {
            if let Some((event, parameter0, parameter1)) = event {
                self.host
                    .dispatch_native_event(event, parameter0, parameter1)?;
            }
            Ok(None)
        } else {
            Ok(event)
        }
    }

    pub fn route_pointer_event(
        &mut self,
        x: i32,
        y: i32,
        pressed: bool,
    ) -> Result<Option<(i32, i32, i32)>> {
        let event = self.host.route_pointer_event(x, y, pressed)?;
        if self.native_entry {
            if let Some((event, parameter0, parameter1)) = event {
                self.host
                    .dispatch_native_event(event, parameter0, parameter1)?;
            }
            Ok(None)
        } else {
            Ok(event)
        }
    }

    pub fn route_text_input(&mut self, text: &str) -> Result<Option<(i32, i32, i32)>> {
        let event = self.host.route_text_input(text)?;
        if self.native_entry {
            if let Some((event, parameter0, parameter1)) = event {
                self.host
                    .dispatch_native_event(event, parameter0, parameter1)?;
            }
            Ok(None)
        } else {
            Ok(event)
        }
    }

    fn run_frames(&mut self) -> Result<()> {
        while !self.frames.is_empty() {
            if self.instruction_count >= self.limits.max_mr_instructions {
                return Err(crate::Error::ResourceLimit(format!(
                    "MR instruction limit {} exhausted",
                    self.limits.max_mr_instructions
                )));
            }
            self.instruction_count += 1;
            if let Err(error) = self.step()
                && !self.recover_protected(&error)
            {
                let context = self.frames.last().map(|frame| {
                    let prototype = &frame.closure.prototype;
                    let source = prototype
                        .source
                        .as_deref()
                        .map(String::from_utf8_lossy)
                        .unwrap_or_else(|| "?".into());
                    format!(
                        "{source}: line-defined {}, instruction {}/{}",
                        prototype.line_defined,
                        frame.pc,
                        prototype.code.len()
                    )
                });
                return Err(match context {
                    Some(context) => crate::Error::MrFault(format!("{context}: {error}")),
                    None => error,
                });
            }
        }
        Ok(())
    }

    fn step(&mut self) -> Result<()> {
        let frame_index = self.frames.len() - 1;
        let instruction = {
            let frame = &mut self.frames[frame_index];
            let instruction = frame
                .closure
                .prototype
                .code
                .get(frame.pc)
                .copied()
                .ok_or_else(|| crate::Error::MrFault("program counter left the function".into()))?;
            frame.pc += 1;
            instruction
        };

        let opcode = (instruction & 0x3f) as u8;
        let a = (instruction >> 24) as usize;
        let b = ((instruction >> 15) & 0x1ff) as usize;
        let c = ((instruction >> 6) & 0x1ff) as usize;
        let bx = ((instruction >> 6) & 0x3ffff) as usize;
        let sbx = bx as isize - 131_071;

        if std::env::var_os("SKYENGINE_TRACE_MR_INSTRUCTIONS").is_some() {
            let frame = &self.frames[frame_index];
            let source = frame
                .closure
                .prototype
                .source
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_else(|| "?".into());
            let registers = frame
                .registers
                .iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    let value = value.borrow();
                    (!matches!(*value, Value::Nil))
                        .then(|| format!("r{index}={}", trace_value(&value)))
                })
                .collect::<Vec<_>>()
                .join(" ");
            let operand = if matches!(opcode, 5 | 7) {
                self.constant_bytes(frame_index, bx)
                    .ok()
                    .map(|name| format!(" global={:?}", String::from_utf8_lossy(&name)))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            eprintln!(
                "[mr-step] {source} pc={} op={opcode} a={a} b={b} c={c} bx={bx}{operand} {registers}",
                frame.pc - 1
            );
        }

        match opcode {
            0 => self.set_register(frame_index, a, self.register(frame_index, b)?)?,
            1 => {
                let value = self.constant(frame_index, bx)?;
                self.set_register(frame_index, a, value)?;
            }
            2 => {
                self.set_register(frame_index, a, Value::Boolean(b != 0))?;
                if c != 0 {
                    self.skip(frame_index, 1)?;
                }
            }
            3 => {
                for register in a..=b {
                    self.set_register(frame_index, register, Value::Nil)?;
                }
            }
            4 => {
                let value = self.frames[frame_index]
                    .closure
                    .upvalues
                    .get(b)
                    .ok_or_else(|| crate::Error::MrFault(format!("invalid upvalue {b}")))?
                    .borrow()
                    .clone();
                self.set_register(frame_index, a, value)?;
            }
            5 => {
                let name = self.constant_bytes(frame_index, bx)?;
                self.set_register(frame_index, a, self.global(&name))?;
            }
            6 => {
                let table = self.register(frame_index, b)?;
                let key = self.rk(frame_index, c)?;
                let value = table_get(&table, &key)?;
                self.set_register(frame_index, a, value)?;
            }
            7 => {
                let name = self.constant_bytes(frame_index, bx)?;
                let value = self.register(frame_index, a)?;
                self.set_global(&name, value)?;
            }
            8 => {
                let value = self.register(frame_index, a)?;
                let upvalue = self.frames[frame_index]
                    .closure
                    .upvalues
                    .get(b)
                    .ok_or_else(|| crate::Error::MrFault(format!("invalid upvalue {b}")))?;
                *upvalue.borrow_mut() = value;
            }
            9 => {
                let table = self.register(frame_index, a)?;
                let key = self.rk(frame_index, b)?;
                let value = self.rk(frame_index, c)?;
                table_set(&table, key, value)?;
            }
            10 => self.set_register(frame_index, a, Value::Table(Table::new()))?,
            11 => {
                let object = self.register(frame_index, b)?;
                let key = self.rk(frame_index, c)?;
                let method = table_get(&object, &key)?;
                self.set_register(frame_index, a + 1, object)?;
                self.set_register(frame_index, a, method)?;
            }
            12..=16 | 36..=38 => {
                let left = self.rk(frame_index, b)?;
                let right = self.rk(frame_index, c)?;
                let profile = self.frames[frame_index].closure.prototype.profile;
                let value = arithmetic(profile, opcode, &left, &right)?;
                self.set_register(frame_index, a, value)?;
            }
            17 => {
                let value = number(&self.register(frame_index, b)?)?;
                self.set_register(frame_index, a, Value::Number(-value))?;
            }
            18 => {
                let value = !self.register(frame_index, b)?.truthy();
                self.set_register(frame_index, a, Value::Boolean(value))?;
            }
            19 => {
                let mut output = Vec::new();
                for register in b..=c {
                    let value = self.register(frame_index, register)?;
                    let bytes = value.bytes().ok_or_else(|| {
                        crate::Error::MrFault(format!("cannot concatenate {value:?}"))
                    })?;
                    output.extend_from_slice(&bytes);
                }
                self.set_register(frame_index, a, Value::Bytes(output.into()))?;
            }
            20 => self.skip(frame_index, sbx)?,
            21..=23 => {
                let left = self.rk(frame_index, b)?;
                let right = self.rk(frame_index, c)?;
                let result = compare(opcode, &left, &right)?;
                if std::env::var_os("SKYENGINE_TRACE_MR_INSTRUCTIONS").is_some() {
                    eprintln!(
                        "[mr-compare] {} {} {} -> {result}",
                        trace_value(&left),
                        match opcode {
                            21 => "==",
                            22 => "<",
                            _ => "<=",
                        },
                        trace_value(&right)
                    );
                }
                if result != (a != 0) {
                    self.skip(frame_index, 1)?;
                }
            }
            24 => {
                let value = self.register(frame_index, b)?;
                if value.truthy() == (c != 0) {
                    self.set_register(frame_index, a, value)?;
                } else {
                    self.skip(frame_index, 1)?;
                }
            }
            25 => self.call_instruction(frame_index, a, b, c, false)?,
            26 => self.call_instruction(frame_index, a, b, c, true)?,
            27 => {
                let count = if b == 0 {
                    self.frames[frame_index].top.saturating_sub(a)
                } else {
                    b - 1
                };
                let values = self.register_range(frame_index, a, count)?;
                self.return_from_frame(values)?;
            }
            28 => {
                let index = number(&self.register(frame_index, a)?)?
                    + number(&self.register(frame_index, a + 2)?)?;
                let limit = number(&self.register(frame_index, a + 1)?)?;
                let step = number(&self.register(frame_index, a + 2)?)?;
                self.set_register(frame_index, a, Value::Number(index))?;
                if (step > 0.0 && index <= limit) || (step <= 0.0 && index >= limit) {
                    self.skip(frame_index, sbx)?;
                }
            }
            29 => self.iterator_loop(frame_index, a, c)?,
            30 => {
                if matches!(self.register(frame_index, a)?, Value::Table(_)) {
                    let state = self.register(frame_index, a)?;
                    self.set_register(frame_index, a, Value::Native("next"))?;
                    self.set_register(frame_index, a + 1, state)?;
                }
                self.skip(frame_index, sbx)?;
            }
            31 => {
                let table = self.register(frame_index, a)?;
                let item_count = bx % 32 + 1;
                let first_index = bx - bx % 32 + 1;
                for item in 0..item_count {
                    table_set(
                        &table,
                        Value::Number((first_index + item) as f64),
                        self.register(frame_index, a + item + 1)?,
                    )?;
                }
            }
            32 => {
                let table = self.register(frame_index, a)?;
                let count = self.frames[frame_index].top.saturating_sub(a + 1);
                let first_index = bx - bx % 32 + 1;
                for item in 0..count {
                    table_set(
                        &table,
                        Value::Number((first_index + item) as f64),
                        self.register(frame_index, a + item + 1)?,
                    )?;
                }
            }
            33 => self.close_upvalues(frame_index, a)?,
            34 => self.create_closure(frame_index, a, bx)?,
            35 => {
                let value = integer_number(&self.register(frame_index, b)?)?;
                self.set_register(frame_index, a, Value::Number((!value) as f64))?;
            }
            _ => {
                return Err(crate::Error::MrFault(format!(
                    "unsupported opcode {opcode}"
                )));
            }
        }
        Ok(())
    }

    fn call_instruction(
        &mut self,
        frame_index: usize,
        a: usize,
        b: usize,
        c: usize,
        tail: bool,
    ) -> Result<()> {
        let argument_count = if b == 0 {
            self.frames[frame_index].top.saturating_sub(a + 1)
        } else {
            b - 1
        };
        let function = self.register(frame_index, a)?;
        let args = self.register_range(frame_index, a + 1, argument_count)?;
        let target = if tail {
            self.frames[frame_index].return_target
        } else {
            Some(ReturnTarget {
                register: a,
                expected: if c == 0 { None } else { Some(c - 1) },
                protected: false,
            })
        };

        if tail {
            self.frames.pop().expect("active CALL frame");
        }
        match self.call_value(function, args, target, false)? {
            CallResult::Pushed => {}
            CallResult::Immediate(values) => {
                if tail {
                    self.finish_return(target, values)?;
                } else {
                    self.assign_results(frame_index, target.expect("call target"), values)?;
                }
            }
        }
        Ok(())
    }

    fn call_value(
        &mut self,
        function: Value,
        args: Vec<Value>,
        target: Option<ReturnTarget>,
        protected: bool,
    ) -> Result<CallResult> {
        match function {
            Value::Closure(closure) => {
                let target = target.map(|mut target| {
                    target.protected |= protected;
                    target
                });
                self.push_frame(closure, args, target)?;
                Ok(CallResult::Pushed)
            }
            Value::Native("dofile") => {
                let name = args
                    .first()
                    .and_then(Value::bytes)
                    .ok_or_else(|| crate::Error::MrFault("dofile expects a file name".into()))?;
                let source = self.host.package.read_named(&name)?;
                let chunk = MrChunk::load(&source, &self.limits)?;
                let closure = Rc::new(Closure {
                    prototype: chunk.root,
                    upvalues: Vec::new(),
                });
                let target = target.map(|mut target| {
                    target.protected |= protected;
                    target
                });
                self.push_frame(closure, Vec::new(), target)?;
                Ok(CallResult::Pushed)
            }
            Value::Native("pcall" | "_pCall") => self.protected_call(args, target),
            Value::Native(name) => {
                let values = match self.call_native(name, &args) {
                    Ok(values) => values,
                    Err(error) if protected => {
                        vec![Value::Boolean(false), error_value(&error)]
                    }
                    Err(error) => return Err(error),
                };
                Ok(CallResult::Immediate(values))
            }
            other => Err(crate::Error::MrFault(format!(
                "attempt to call {other:?} with arguments {args:?}"
            ))),
        }
    }

    fn protected_call(
        &mut self,
        mut args: Vec<Value>,
        target: Option<ReturnTarget>,
    ) -> Result<CallResult> {
        if args.is_empty() {
            return Ok(CallResult::Immediate(vec![
                Value::Boolean(false),
                bytes(b"pcall expects a function"),
            ]));
        }
        let function = args.remove(0);
        match function {
            Value::Closure(closure) => {
                let mut target = target.unwrap_or(ReturnTarget {
                    register: 0,
                    expected: None,
                    protected: true,
                });
                target.protected = true;
                self.push_frame(closure, args, Some(target))?;
                Ok(CallResult::Pushed)
            }
            Value::Native(name) => match self.call_native(name, &args) {
                Ok(mut values) => {
                    values.insert(0, Value::Boolean(true));
                    Ok(CallResult::Immediate(values))
                }
                Err(error) => Ok(CallResult::Immediate(vec![
                    Value::Boolean(false),
                    error_value(&error),
                ])),
            },
            other => Ok(CallResult::Immediate(vec![
                Value::Boolean(false),
                bytes(format!("attempt to call {other:?}").as_bytes()),
            ])),
        }
    }

    fn push_frame(
        &mut self,
        closure: ClosureRef,
        args: Vec<Value>,
        return_target: Option<ReturnTarget>,
    ) -> Result<()> {
        if self.frames.len() >= 1024 {
            return Err(crate::Error::ResourceLimit(
                "MR call depth exceeds 1024".into(),
            ));
        }
        let prototype = &closure.prototype;
        let registers = (0..prototype.max_stack_size)
            .map(|_| Value::Nil.cell())
            .collect::<Vec<_>>();
        for (index, value) in args.iter().take(registers.len()).cloned().enumerate() {
            *registers[index].borrow_mut() = value;
        }
        if prototype.is_vararg && usize::from(prototype.parameter_count) < registers.len() {
            let varargs = Table::new();
            let fixed = usize::from(prototype.parameter_count);
            for (index, value) in args.iter().skip(fixed).cloned().enumerate() {
                varargs
                    .borrow_mut()
                    .set(Value::Number((index + 1) as f64), value);
            }
            varargs.borrow_mut().set(
                bytes(b"n"),
                Value::Number(args.len().saturating_sub(fixed) as f64),
            );
            *registers[fixed].borrow_mut() = Value::Table(varargs);
        }
        self.frames.push(Frame {
            closure,
            registers,
            pc: 0,
            top: args.len(),
            return_target,
        });
        Ok(())
    }

    fn return_from_frame(&mut self, values: Vec<Value>) -> Result<()> {
        let frame = self.frames.pop().expect("active frame");
        self.finish_return(frame.return_target, values)
    }

    fn finish_return(
        &mut self,
        target: Option<ReturnTarget>,
        mut values: Vec<Value>,
    ) -> Result<()> {
        let Some(target) = target else {
            self.final_values = values;
            return Ok(());
        };
        if target.protected {
            values.insert(0, Value::Boolean(true));
        }
        let caller = self
            .frames
            .len()
            .checked_sub(1)
            .ok_or_else(|| crate::Error::MrFault("return target has no caller".into()))?;
        self.assign_results(caller, target, values)
    }

    fn assign_results(
        &mut self,
        frame_index: usize,
        target: ReturnTarget,
        values: Vec<Value>,
    ) -> Result<()> {
        let count = target.expected.unwrap_or(values.len());
        for index in 0..count {
            self.set_register(
                frame_index,
                target.register + index,
                values.get(index).cloned().unwrap_or(Value::Nil),
            )?;
        }
        if target.expected.is_none() {
            self.frames[frame_index].top = target.register + count;
        }
        Ok(())
    }

    fn recover_protected(&mut self, error: &crate::Error) -> bool {
        let Some(index) = self
            .frames
            .iter()
            .rposition(|frame| frame.return_target.is_some_and(|target| target.protected))
        else {
            return false;
        };
        let target = self.frames[index].return_target.expect("protected target");
        self.frames.truncate(index);
        let Some(caller) = self.frames.len().checked_sub(1) else {
            return false;
        };
        self.assign_results(
            caller,
            target,
            vec![Value::Boolean(false), error_value(error)],
        )
        .is_ok()
    }

    fn create_closure(&mut self, frame_index: usize, a: usize, bx: usize) -> Result<()> {
        let prototype = self.frames[frame_index]
            .closure
            .prototype
            .prototypes
            .get(bx)
            .cloned()
            .ok_or_else(|| crate::Error::MrFault(format!("invalid prototype {bx}")))?;
        let mut upvalues = Vec::with_capacity(usize::from(prototype.upvalue_count));
        for _ in 0..prototype.upvalue_count {
            let pseudo = {
                let frame = &mut self.frames[frame_index];
                let instruction = frame
                    .closure
                    .prototype
                    .code
                    .get(frame.pc)
                    .copied()
                    .ok_or_else(|| crate::Error::MrFault("closure capture is truncated".into()))?;
                frame.pc += 1;
                instruction
            };
            let opcode = (pseudo & 0x3f) as u8;
            let b = ((pseudo >> 15) & 0x1ff) as usize;
            let cell = match opcode {
                0 => self.frames[frame_index]
                    .registers
                    .get(b)
                    .cloned()
                    .ok_or_else(|| {
                        crate::Error::MrFault(format!("invalid capture register {b}"))
                    })?,
                4 => self.frames[frame_index]
                    .closure
                    .upvalues
                    .get(b)
                    .cloned()
                    .ok_or_else(|| {
                        crate::Error::MrFault(format!("invalid captured upvalue {b}"))
                    })?,
                _ => {
                    return Err(crate::Error::MrFault(format!(
                        "opcode {opcode} cannot describe a closure capture"
                    )));
                }
            };
            upvalues.push(cell);
        }
        self.set_register(
            frame_index,
            a,
            Value::Closure(Rc::new(Closure {
                prototype,
                upvalues,
            })),
        )
    }

    fn close_upvalues(&mut self, frame_index: usize, start: usize) -> Result<()> {
        let registers = self.frames[frame_index]
            .registers
            .get_mut(start..)
            .ok_or_else(|| crate::Error::MrFault(format!("invalid CLOSE register {start}")))?;
        for register in registers {
            let value = register.borrow().clone();
            *register = value.cell();
        }
        Ok(())
    }

    fn iterator_loop(&mut self, frame_index: usize, a: usize, c: usize) -> Result<()> {
        let function = self.register(frame_index, a)?;
        let args = vec![
            self.register(frame_index, a + 1)?,
            self.register(frame_index, a + 2)?,
        ];
        let values = match self.call_value(function, args, None, false)? {
            CallResult::Immediate(values) => values,
            CallResult::Pushed => {
                return Err(crate::Error::MrFault(
                    "script iterators are not supported by TFORLOOP".into(),
                ));
            }
        };
        for index in 0..=c {
            self.set_register(
                frame_index,
                a + 2 + index,
                values.get(index).cloned().unwrap_or(Value::Nil),
            )?;
        }
        if !matches!(self.register(frame_index, a + 2)?, Value::Nil) {
            self.set_register(
                frame_index,
                a + 2,
                values.first().cloned().unwrap_or(Value::Nil),
            )?;
        } else {
            self.skip(frame_index, 1)?;
        }
        Ok(())
    }

    fn register(&self, frame: usize, register: usize) -> Result<Value> {
        Ok(self.frames[frame]
            .registers
            .get(register)
            .ok_or_else(|| crate::Error::MrFault(format!("invalid register {register}")))?
            .borrow()
            .clone())
    }

    fn set_register(&mut self, frame: usize, register: usize, value: Value) -> Result<()> {
        let cell = self.frames[frame]
            .registers
            .get(register)
            .ok_or_else(|| crate::Error::MrFault(format!("invalid register {register}")))?;
        *cell.borrow_mut() = value;
        self.frames[frame].top = self.frames[frame].top.max(register + 1);
        Ok(())
    }

    fn register_range(&self, frame: usize, start: usize, count: usize) -> Result<Vec<Value>> {
        (start..start + count)
            .map(|register| self.register(frame, register))
            .collect()
    }

    fn constant(&self, frame: usize, index: usize) -> Result<Value> {
        let constant = self.frames[frame]
            .closure
            .prototype
            .constants
            .get(index)
            .ok_or_else(|| crate::Error::MrFault(format!("invalid constant {index}")))?;
        Ok(match constant {
            Constant::Nil => Value::Nil,
            Constant::Number(value) => Value::Number(*value),
            Constant::Bytes(value) => Value::Bytes(value.clone()),
        })
    }

    fn constant_bytes(&self, frame: usize, index: usize) -> Result<Arc<[u8]>> {
        match self.constant(frame, index)? {
            Value::Bytes(value) => Ok(value),
            other => Err(crate::Error::MrFault(format!(
                "constant {index} is not a string: {other:?}"
            ))),
        }
    }

    fn rk(&self, frame: usize, operand: usize) -> Result<Value> {
        if operand >= RK_LIMIT {
            self.constant(frame, operand - RK_LIMIT)
        } else {
            self.register(frame, operand)
        }
    }

    fn skip(&mut self, frame: usize, amount: isize) -> Result<()> {
        let pc = self.frames[frame].pc as isize + amount;
        if pc < 0 || pc as usize > self.frames[frame].closure.prototype.code.len() {
            return Err(crate::Error::MrFault(format!(
                "jump leaves function at {pc}"
            )));
        }
        self.frames[frame].pc = pc as usize;
        Ok(())
    }

    fn global(&self, name: &[u8]) -> Value {
        self.globals.borrow().get(&bytes(name))
    }

    fn set_global(&self, name: &[u8], value: Value) -> Result<()> {
        if self.globals.borrow_mut().set(bytes(name), value) {
            Ok(())
        } else {
            Err(crate::Error::MrFault("invalid global name".into()))
        }
    }
}

fn trace_prototype(prototype: &Prototype, depth: usize) {
    eprintln!(
        "[mr-prototype] depth={depth} line={} params={} stack={} constants={} code={}",
        prototype.line_defined,
        prototype.parameter_count,
        prototype.max_stack_size,
        prototype.constants.len(),
        prototype.code.len()
    );
    for (index, constant) in prototype.constants.iter().enumerate() {
        let value = match constant {
            Constant::Nil => "nil".into(),
            Constant::Number(number) => number.to_string(),
            Constant::Bytes(bytes) => format!("{:?}", String::from_utf8_lossy(bytes)),
        };
        eprintln!("[mr-constant] depth={depth} index={index} value={value}");
    }
    for (pc, instruction) in prototype.code.iter().copied().enumerate() {
        let opcode = instruction & 0x3f;
        let a = instruction >> 24;
        let b = (instruction >> 15) & 0x1ff;
        let c = (instruction >> 6) & 0x1ff;
        let bx = (instruction >> 6) & 0x3ffff;
        eprintln!("[mr-code] depth={depth} pc={pc} op={opcode} a={a} b={b} c={c} bx={bx}");
    }
    for child in &prototype.prototypes {
        trace_prototype(child, depth + 1);
    }
}

fn trace_values(values: &[Value]) -> String {
    values
        .iter()
        .map(trace_value)
        .collect::<Vec<_>>()
        .join(", ")
}

fn trace_value(value: &Value) -> String {
    match value {
        Value::Bytes(bytes) if bytes.len() > 48 => {
            format!("bytes(len={}, head={:02x?})", bytes.len(), &bytes[..16])
        }
        Value::Table(table) => format!(
            "table:{:p}{}",
            std::rc::Rc::as_ptr(table),
            table.borrow().debug_entries()
        ),
        _ => format!("{value:?}"),
    }
}

fn arithmetic(profile: MrProfile, opcode: u8, left: &Value, right: &Value) -> Result<Value> {
    if opcode >= 36 {
        let left = integer_number(left)?;
        let right = integer_number(right)?;
        return Ok(Value::Number(match opcode {
            36 => left & right,
            37 => left | right,
            38 => left ^ right,
            _ => unreachable!(),
        } as f64));
    }
    let left = number(left)?;
    let right = number(right)?;
    Ok(Value::Number(match opcode {
        12 => left + right,
        13 => left - right,
        14 => left * right,
        15 if profile == MrProfile::V80 => (left / right).trunc(),
        15 => left / right,
        16 => left.powf(right),
        _ => unreachable!(),
    }))
}

fn compare(opcode: u8, left: &Value, right: &Value) -> Result<bool> {
    if opcode == 21 {
        return Ok(left.raw_equal(right));
    }
    let ordering = match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.partial_cmp(right),
        (Value::Bytes(left), Value::Bytes(right)) => Some(left.cmp(right)),
        _ => None,
    }
    .ok_or_else(|| crate::Error::MrFault(format!("cannot compare {left:?} and {right:?}")))?;
    Ok(if opcode == 22 {
        ordering == Ordering::Less
    } else {
        ordering != Ordering::Greater
    })
}

fn table_get(table: &Value, key: &Value) -> Result<Value> {
    match table {
        Value::Table(table) => Ok(table.borrow().get(key)),
        other => Err(crate::Error::MrFault(format!(
            "attempt to index {other:?} with {key:?}"
        ))),
    }
}

fn table_set(table: &Value, key: Value, value: Value) -> Result<()> {
    match table {
        Value::Table(table) if table.borrow_mut().set(key, value) => Ok(()),
        Value::Table(_) => Err(crate::Error::MrFault("nil or NaN table key".into())),
        other => Err(crate::Error::MrFault(format!("attempt to index {other:?}"))),
    }
}

fn table(value: Option<&Value>) -> Result<TableRef> {
    match value {
        Some(Value::Table(table)) => Ok(table.clone()),
        other => Err(crate::Error::MrFault(format!(
            "expected table, got {other:?}"
        ))),
    }
}

fn number(value: &Value) -> Result<f64> {
    value
        .number()
        .ok_or_else(|| crate::Error::MrFault(format!("expected number, got {value:?}")))
}

fn integer_number(value: &Value) -> Result<i64> {
    let number = number(value)?;
    if !number.is_finite() || number < i64::MIN as f64 || number > i64::MAX as f64 {
        return Err(crate::Error::MrFault(format!("invalid integer {number}")));
    }
    Ok(number as i64)
}

fn value_bytes(value: Option<&Value>) -> Result<Arc<[u8]>> {
    value
        .and_then(Value::bytes)
        .ok_or_else(|| crate::Error::MrFault("expected string".into()))
}

fn bytes(value: &[u8]) -> Value {
    Value::Bytes(Arc::from(value))
}

fn error_value(error: &crate::Error) -> Value {
    bytes(error.to_string().as_bytes())
}

#[cfg(test)]
mod tests;
