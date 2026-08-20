use std::sync::Arc;

use crate::{Error, ResourceLimits, Result};

const SIGNATURE: &[u8; 4] = b"\x1bMRP";
const MAX_OPCODE: u8 = 38;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MrProfile {
    V50,
    V80,
}

impl MrProfile {
    pub fn version(self) -> u8 {
        match self {
            Self::V50 => 0x50,
            Self::V80 => 0x80,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Constant {
    Nil,
    Number(f64),
    Bytes(Arc<[u8]>),
}

#[derive(Clone, Debug)]
pub struct LocalVariable {
    pub name: Arc<[u8]>,
    pub start_pc: u32,
    pub end_pc: u32,
}

#[derive(Clone, Debug)]
pub struct Prototype {
    pub source: Option<Arc<[u8]>>,
    pub line_defined: u32,
    pub upvalue_count: u8,
    pub parameter_count: u8,
    pub is_vararg: bool,
    pub max_stack_size: u8,
    pub line_info: Vec<u32>,
    pub locals: Vec<LocalVariable>,
    pub upvalue_names: Vec<Arc<[u8]>>,
    pub constants: Vec<Constant>,
    pub prototypes: Vec<Arc<Prototype>>,
    pub code: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct MrChunk {
    pub profile: MrProfile,
    pub root: Arc<Prototype>,
    pub prototype_count: usize,
}

impl MrChunk {
    pub fn load(bytes: &[u8], limits: &ResourceLimits) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        if reader.take(4)? != SIGNATURE {
            return Err(Error::UnsupportedMr("missing 1B 4D 52 50 signature".into()));
        }
        let version = reader.byte()?;
        let profile = match version {
            0x50 => MrProfile::V50,
            0x80 => MrProfile::V80,
            other => {
                return Err(Error::UnsupportedMr(format!(
                    "MR chunk version {other:#04x}"
                )));
            }
        };
        let endian = reader.byte()?;
        reader.endian = match endian {
            1 => Endian::Little,
            0 => Endian::Big,
            other => {
                return Err(Error::mr_load(
                    reader.offset().saturating_sub(1),
                    format!("invalid endian marker {other}"),
                ));
            }
        };

        if profile == MrProfile::V50 {
            let expected = [4_u8, 4, 4, 6, 8, 9, 9, 8];
            let header_size_count = expected.len();
            for (actual, field_size) in reader.take(header_size_count)?.iter().zip(expected) {
                if *actual != field_size {
                    return Err(Error::mr_load(
                        reader.offset().saturating_sub(header_size_count),
                        "unsupported v0x50 field sizes",
                    ));
                }
            }
            let test_number = reader.f64()?;
            if (test_number - 3.141_592_653_589_793_4e8_f64).abs() > 0.5 {
                return Err(Error::mr_load(
                    reader.offset().saturating_sub(8),
                    "unsupported number representation",
                ));
            }
        }

        let mut budget = LoadBudget::default();
        let root = load_prototype(&mut reader, limits, &mut budget, profile, 0, None)?;
        if reader.offset() != bytes.len() {
            return Err(Error::mr_load(
                reader.offset(),
                format!(
                    "{} trailing bytes after root prototype",
                    bytes.len() - reader.offset()
                ),
            ));
        }
        Ok(Self {
            profile,
            root,
            prototype_count: budget.prototypes,
        })
    }
}

#[derive(Default)]
struct LoadBudget {
    prototypes: usize,
    items: usize,
}

fn load_prototype(
    reader: &mut Reader<'_>,
    limits: &ResourceLimits,
    budget: &mut LoadBudget,
    profile: MrProfile,
    depth: usize,
    parent_source: Option<Arc<[u8]>>,
) -> Result<Arc<Prototype>> {
    if depth >= limits.max_mr_depth {
        return Err(Error::ResourceLimit(format!(
            "MR prototype nesting exceeds {}",
            limits.max_mr_depth
        )));
    }
    budget.prototypes = budget.prototypes.saturating_add(1);
    if budget.prototypes > limits.max_mr_prototypes {
        return Err(Error::ResourceLimit(format!(
            "MR chunk has more than {} prototypes",
            limits.max_mr_prototypes
        )));
    }

    let source = reader.string(limits)?.or(parent_source);
    let line_defined = reader.u32()?;
    let upvalue_count = reader.byte()?;
    let parameter_count = reader.byte()?;
    let is_vararg = reader.byte()? != 0;
    let max_stack_size = reader.byte()?;
    if max_stack_size == 0 || max_stack_size > 250 {
        return Err(Error::mr_load(
            reader.offset().saturating_sub(1),
            format!("invalid register count {max_stack_size}"),
        ));
    }

    let line_count = reader.count(limits, budget, "line records")?;
    let mut line_info = Vec::with_capacity(line_count);
    for _ in 0..line_count {
        line_info.push(reader.u32()?);
    }

    let local_count = reader.count(limits, budget, "local variables")?;
    let mut locals = Vec::with_capacity(local_count);
    for _ in 0..local_count {
        let name = reader
            .string(limits)?
            .unwrap_or_else(|| Arc::from(&b""[..]));
        let start_pc = reader.u32()?;
        let end_pc = reader.u32()?;
        locals.push(LocalVariable {
            name,
            start_pc,
            end_pc,
        });
    }

    let upvalue_name_count = reader.count(limits, budget, "upvalue names")?;
    if upvalue_name_count != 0 && upvalue_name_count != usize::from(upvalue_count) {
        return Err(Error::mr_load(
            reader.offset(),
            format!("prototype declares {upvalue_count} upvalues but names {upvalue_name_count}"),
        ));
    }
    let mut upvalue_names = Vec::with_capacity(upvalue_name_count);
    for _ in 0..upvalue_name_count {
        upvalue_names.push(
            reader
                .string(limits)?
                .unwrap_or_else(|| Arc::from(&b""[..])),
        );
    }

    let constant_count = reader.count(limits, budget, "constants")?;
    let mut constants = Vec::with_capacity(constant_count);
    for _ in 0..constant_count {
        let tag_offset = reader.offset();
        let value = match reader.byte()? {
            0 => Constant::Nil,
            3 => Constant::Number(match profile {
                MrProfile::V50 => reader.f64()?,
                MrProfile::V80 => f64::from(reader.i32()?),
            }),
            4 => Constant::Bytes(reader.string(limits)?.ok_or_else(|| {
                Error::mr_load(tag_offset, "string constant has a null string record")
            })?),
            tag => {
                return Err(Error::mr_load(
                    tag_offset,
                    format!("unsupported constant tag {tag}"),
                ));
            }
        };
        constants.push(value);
    }

    let child_count = reader.count(limits, budget, "nested prototypes")?;
    let mut prototypes = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        prototypes.push(load_prototype(
            reader,
            limits,
            budget,
            profile,
            depth + 1,
            source.clone(),
        )?);
    }

    let code_count = reader.count(limits, budget, "instructions")?;
    let mut code = Vec::with_capacity(code_count);
    for index in 0..code_count {
        let instruction_offset = reader.offset();
        let instruction = reader.u32()?;
        let opcode = (instruction & 0x3f) as u8;
        if opcode > MAX_OPCODE {
            return Err(Error::mr_load(
                instruction_offset,
                format!("unknown opcode {opcode} at instruction {index}"),
            ));
        }
        code.push(instruction);
    }

    validate_code(
        &code,
        constants.len(),
        prototypes.len(),
        max_stack_size,
        upvalue_count,
    )?;

    Ok(Arc::new(Prototype {
        source,
        line_defined,
        upvalue_count,
        parameter_count,
        is_vararg,
        max_stack_size,
        line_info,
        locals,
        upvalue_names,
        constants,
        prototypes,
        code,
    }))
}

fn validate_code(
    code: &[u32],
    constant_count: usize,
    prototype_count: usize,
    max_stack_size: u8,
    upvalue_count: u8,
) -> Result<()> {
    let registers = usize::from(max_stack_size);
    for (pc, instruction) in code.iter().copied().enumerate() {
        let opcode = (instruction & 0x3f) as u8;
        let a = (instruction >> 24) as usize;
        let b = ((instruction >> 15) & 0x1ff) as usize;
        let c = ((instruction >> 6) & 0x1ff) as usize;
        let bx = ((instruction >> 6) & 0x3ffff) as usize;
        if matches!(
            opcode,
            0..=19 | 21..=34 | 35..=38
        ) && a >= registers
        {
            return Err(Error::mr_load(
                pc * 4,
                format!("register A {a} is out of range"),
            ));
        }
        if matches!(opcode, 1 | 5 | 7) && bx >= constant_count {
            return Err(Error::mr_load(
                pc * 4,
                format!("constant index {bx} is out of range"),
            ));
        }
        if opcode == 34 && bx >= prototype_count {
            return Err(Error::mr_load(
                pc * 4,
                format!("prototype index {bx} is out of range"),
            ));
        }
        if opcode == 4 && b >= usize::from(upvalue_count) {
            return Err(Error::mr_load(
                pc * 4,
                format!("upvalue index {b} is out of range"),
            ));
        }
        for operand in [b, c] {
            if operand >= 250
                && operand - 250 >= constant_count
                && matches!(opcode, 6 | 9 | 11..=16 | 21..=23 | 36..=38)
            {
                return Err(Error::mr_load(
                    pc * 4,
                    format!("RK constant index {} is out of range", operand - 250),
                ));
            }
        }
        if matches!(opcode, 20 | 28 | 30) {
            let target = pc as i64 + 1 + bx as i64 - 131_071;
            if target < 0 || target > code.len() as i64 {
                return Err(Error::mr_load(
                    pc * 4,
                    format!("jump target {target} is out of range"),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
    endian: Endian,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            cursor: 0,
            endian: Endian::Little,
        }
    }

    fn offset(&self) -> usize {
        self.cursor
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let start = self.cursor;
        let end = start
            .checked_add(len)
            .ok_or_else(|| Error::mr_load(start, "range overflow"))?;
        let bytes = self
            .bytes
            .get(start..end)
            .ok_or_else(|| Error::mr_load(start, format!("truncated {len}-byte field")))?;
        self.cursor = end;
        Ok(bytes)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        let raw: [u8; 4] = self.take(4)?.try_into().expect("fixed-size slice");
        Ok(match self.endian {
            Endian::Little => u32::from_le_bytes(raw),
            Endian::Big => u32::from_be_bytes(raw),
        })
    }

    fn i32(&mut self) -> Result<i32> {
        let raw: [u8; 4] = self.take(4)?.try_into().expect("fixed-size slice");
        Ok(match self.endian {
            Endian::Little => i32::from_le_bytes(raw),
            Endian::Big => i32::from_be_bytes(raw),
        })
    }

    fn f64(&mut self) -> Result<f64> {
        let raw: [u8; 8] = self.take(8)?.try_into().expect("fixed-size slice");
        Ok(match self.endian {
            Endian::Little => f64::from_le_bytes(raw),
            Endian::Big => f64::from_be_bytes(raw),
        })
    }

    fn count(
        &mut self,
        limits: &ResourceLimits,
        budget: &mut LoadBudget,
        kind: &str,
    ) -> Result<usize> {
        let offset = self.offset();
        let count = usize::try_from(self.u32()?)
            .map_err(|_| Error::mr_load(offset, format!("{kind} count does not fit the host")))?;
        budget.items = budget.items.saturating_add(count);
        if budget.items > limits.max_mr_items {
            return Err(Error::ResourceLimit(format!(
                "MR chunk items exceed {} while loading {kind}",
                limits.max_mr_items
            )));
        }
        Ok(count)
    }

    fn string(&mut self, limits: &ResourceLimits) -> Result<Option<Arc<[u8]>>> {
        let offset = self.offset();
        let len = usize::try_from(self.u32()?)
            .map_err(|_| Error::mr_load(offset, "string length does not fit the host"))?;
        if len == 0 {
            return Ok(None);
        }
        if len > limits.max_mr_string_len {
            return Err(Error::ResourceLimit(format!(
                "MR string is {len} bytes (limit {})",
                limits.max_mr_string_len
            )));
        }
        let bytes = self.take(len)?;
        if bytes.last() != Some(&0) {
            return Err(Error::mr_load(offset, "MR string is not NUL terminated"));
        }
        Ok(Some(Arc::from(&bytes[..len - 1])))
    }
}
