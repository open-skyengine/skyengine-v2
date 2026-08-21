use std::{cmp::Ordering, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use crate::{
    Framebuffer, Package, PlatformDisplay, ResourceLimits, Result, arm::ExtLifecycleRequest,
};

use super::{
    chunk::{Constant, MrChunk, Prototype},
    host::MrHost,
    value::{Cell, Closure, ClosureRef, Table, TableRef, Value},
};

const RK_LIMIT: usize = 250;
const SIGNATURE: &[u8; 4] = b"\x1bMRP";

pub struct MrVm {
    globals: TableRef,
    frames: Vec<Frame>,
    host: MrHost,
    limits: ResourceLimits,
    instruction_count: u64,
    final_values: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LifecycleOutcome {
    Continue,
    ExitRequested,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        package: Arc<Package>,
        framebuffer: Framebuffer,
        display: Box<dyn PlatformDisplay>,
        work_dir: PathBuf,
        font: Arc<[u8]>,
        memory_limit: u32,
        limits: ResourceLimits,
    ) -> Self {
        let globals = Table::new();
        let mut vm = Self {
            globals,
            frames: Vec::new(),
            host: MrHost::new(package, framebuffer, display, work_dir, font, memory_limit),
            limits,
            instruction_count: 0,
            final_values: Vec::new(),
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
        self.host.dispatch_native_timer()
    }

    pub fn run_entry(&mut self, entry: &[u8]) -> Result<()> {
        self.host.set_current_entry(entry);
        let bytes = self.host.package.read_named(entry)?;
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

    pub(crate) fn process_lifecycle_request(&mut self) -> Result<LifecycleOutcome> {
        let Some(request) = self.host.lifecycle_request()? else {
            return Ok(LifecycleOutcome::Continue);
        };
        match request {
            ExtLifecycleRequest::Restart { package, entry } => {
                let package = self.host.prepare_restart(&package, &entry)?;
                self.globals = Table::new();
                self.frames.clear();
                self.instruction_count = 0;
                self.final_values.clear();
                self.host.reset_for_restart(package, &entry);
                self.register_libraries();
                self.run_entry(&entry)?;
            }
            ExtLifecycleRequest::Exit => return Ok(LifecycleOutcome::ExitRequested),
        }
        Ok(LifecycleOutcome::Continue)
    }

    pub fn route_key_event(&mut self, code: i32, pressed: bool) -> Option<(i32, i32, i32)> {
        self.host.route_key_event(code, pressed)
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
                let value = arithmetic(opcode, &left, &right)?;
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
            self.frames.pop();
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
            Value::Native("pcall") => self.protected_call(args, target),
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

    fn native(&self, name: &'static str) {
        self.globals
            .borrow_mut()
            .set(bytes(name.as_bytes()), Value::Native(name));
    }

    fn register_libraries(&mut self) {
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

    fn call_native(&mut self, name: &'static str, args: &[Value]) -> Result<Vec<Value>> {
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

    fn file_exist(&self, args: &[Value]) -> Result<Vec<Value>> {
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

fn arithmetic(opcode: u8, left: &Value, right: &Value) -> Result<Value> {
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

fn native_tonumber(args: &[Value]) -> Value {
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

fn integer_binary(args: &[Value], operation: impl FnOnce(i64, i64) -> i64) -> Result<Vec<Value>> {
    let left = integer_number(args.first().unwrap_or(&Value::Nil))?;
    let right = integer_number(args.get(1).unwrap_or(&Value::Nil))?;
    Ok(vec![Value::Number(operation(left, right) as f64)])
}

fn string_byte(args: &[Value]) -> Result<Vec<Value>> {
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

fn string_char(args: &[Value]) -> Result<Vec<Value>> {
    let mut output = Vec::with_capacity(args.len());
    for value in args {
        output.push(integer_number(value)?.clamp(0, 255) as u8);
    }
    Ok(vec![Value::Bytes(output.into())])
}

fn string_clen(args: &[Value]) -> Result<Vec<Value>> {
    let string = value_bytes(args.first())?;
    let len = string
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(string.len());
    Ok(vec![Value::Number(len as f64)])
}

fn string_cstr(args: &[Value]) -> Result<Vec<Value>> {
    let string = value_bytes(args.first())?;
    let len = string
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(string.len());
    Ok(vec![Value::Bytes(Arc::from(&string[..len]))])
}

fn string_sub(args: &[Value]) -> Result<Vec<Value>> {
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

fn string_sub_value(args: &[Value]) -> Result<Vec<Value>> {
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

fn string_find(args: &[Value]) -> Result<Vec<Value>> {
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

fn string_format(args: &[Value]) -> Result<Vec<Value>> {
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

fn string_rep(args: &[Value]) -> Result<Vec<Value>> {
    let string = value_bytes(args.first())?;
    let count = usize::try_from(integer_number(args.get(1).unwrap_or(&Value::Nil))?).unwrap_or(0);
    let mut output = Vec::with_capacity(string.len().saturating_mul(count));
    for _ in 0..count {
        output.extend_from_slice(&string);
    }
    Ok(vec![Value::Bytes(output.into())])
}

fn string_case(args: &[Value], upper: bool) -> Result<Vec<Value>> {
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

fn string_pack(args: &[Value]) -> Result<Vec<Value>> {
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

fn string_unpack(args: &[Value]) -> Result<Vec<Value>> {
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

fn table_insert(args: &[Value]) -> Result<Vec<Value>> {
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

fn table_remove(args: &[Value]) -> Result<Vec<Value>> {
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

fn table_concat(args: &[Value]) -> Result<Vec<Value>> {
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

fn lua_index(value: Option<&Value>, len: usize, default: i64) -> Result<usize> {
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

fn safe_work_path(work_dir: &std::path::Path, bytes: &[u8]) -> Option<PathBuf> {
    let path = std::str::from_utf8(bytes).ok()?;
    let path = std::path::Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(work_dir.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_insert_shifts_sequence_values() {
        let table = Table::new();
        table.borrow_mut().set(Value::Number(1.0), bytes(b"a"));
        table.borrow_mut().set(Value::Number(2.0), bytes(b"c"));
        table_insert(&[Value::Table(table.clone()), Value::Number(2.0), bytes(b"b")]).unwrap();
        assert_eq!(table.borrow().sequence_len(), 3);
        assert!(
            table
                .borrow()
                .get(&Value::Number(2.0))
                .raw_equal(&bytes(b"b"))
        );
    }

    #[test]
    fn native_number_supports_radix() {
        assert!(
            native_tonumber(&[bytes(b"7f"), Value::Number(16.0)]).raw_equal(&Value::Number(127.0))
        );
        assert!(matches!(
            native_tonumber(&[bytes(b"not-a-number")]),
            Value::Nil
        ));
    }

    #[test]
    fn c_string_helpers_stop_at_nul() {
        let value = bytes(b"abc\0def");
        assert_eq!(
            string_clen(std::slice::from_ref(&value)).unwrap()[0].number(),
            Some(3.0)
        );
        assert!(string_cstr(&[value]).unwrap()[0].raw_equal(&bytes(b"abc")));
    }

    #[test]
    fn sub_value_splits_numbers_and_byte_strings_into_little_endian_words() {
        let number_bits = 0x1122_3344_aabb_ccdd_u64;
        let number = string_sub_value(&[Value::Number(f64::from_bits(number_bits))]).unwrap();
        assert_eq!(number[0].number(), Some(0xaabb_ccdd_u32 as f64));
        assert_eq!(number[1].number(), Some(0x1122_3344_u32 as f64));

        let string = string_sub_value(&[bytes(b"unknow")]).unwrap();
        assert_eq!(string[0].number(), Some(0x6e6b_6e75_u32 as f64));
        assert_eq!(string[1].number(), Some(0x0000_776f_u32 as f64));
    }
}
