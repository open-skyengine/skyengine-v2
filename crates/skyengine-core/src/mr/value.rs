use std::{cell::RefCell, fmt, rc::Rc, sync::Arc};

use super::chunk::Prototype;

pub type Cell = Rc<RefCell<Value>>;
pub type TableRef = Rc<RefCell<Table>>;
pub type ClosureRef = Rc<Closure>;

#[derive(Clone)]
pub enum Value {
    Nil,
    Boolean(bool),
    Number(f64),
    Bytes(Arc<[u8]>),
    Table(TableRef),
    Closure(ClosureRef),
    Native(&'static str),
}

impl fmt::Debug for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => formatter.write_str("nil"),
            Self::Boolean(value) => value.fmt(formatter),
            Self::Number(value) => value.fmt(formatter),
            Self::Bytes(value) => write!(formatter, "{:?}", String::from_utf8_lossy(value)),
            Self::Table(value) => write!(formatter, "table:{:p}", Rc::as_ptr(value)),
            Self::Closure(value) => write!(formatter, "closure:{:p}", Rc::as_ptr(value)),
            Self::Native(name) => write!(formatter, "native:{name}"),
        }
    }
}

impl Value {
    pub fn cell(self) -> Cell {
        Rc::new(RefCell::new(self))
    }

    pub fn truthy(&self) -> bool {
        !matches!(self, Self::Nil | Self::Boolean(false))
    }

    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Bytes(value) => std::str::from_utf8(value).ok()?.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn bytes(&self) -> Option<Arc<[u8]>> {
        match self {
            Self::Bytes(value) => Some(value.clone()),
            Self::Number(value) => Some(Arc::from(format_number(*value).into_bytes())),
            Self::Boolean(value) => {
                Some(Arc::from(if *value { &b"true"[..] } else { &b"false"[..] }))
            }
            Self::Nil => Some(Arc::from(&b"nil"[..])),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static [u8] {
        match self {
            Self::Nil => b"nil",
            Self::Boolean(_) => b"boolean",
            Self::Number(_) => b"number",
            Self::Bytes(_) => b"string",
            Self::Table(_) => b"table",
            Self::Closure(_) | Self::Native(_) => b"function",
        }
    }

    pub fn key(&self) -> Option<Key> {
        match self {
            Self::Nil => None,
            Self::Boolean(value) => Some(Key::Boolean(*value)),
            Self::Number(value) if !value.is_nan() => {
                let normalized = if *value == 0.0 { 0.0 } else { *value };
                Some(Key::Number(normalized.to_bits()))
            }
            Self::Bytes(value) => Some(Key::Bytes(value.clone())),
            Self::Table(value) => Some(Key::Object(Rc::as_ptr(value) as usize)),
            Self::Closure(value) => Some(Key::Object(Rc::as_ptr(value) as usize)),
            Self::Native(name) => Some(Key::Native(name)),
            Self::Number(_) => None,
        }
    }

    pub fn raw_equal(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Boolean(left), Self::Boolean(right)) => left == right,
            (Self::Number(left), Self::Number(right)) => left == right,
            (Self::Bytes(left), Self::Bytes(right)) => left == right,
            (Self::Table(left), Self::Table(right)) => Rc::ptr_eq(left, right),
            (Self::Closure(left), Self::Closure(right)) => Rc::ptr_eq(left, right),
            (Self::Native(left), Self::Native(right)) => left == right,
            _ => false,
        }
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Key {
    Boolean(bool),
    Number(u64),
    Bytes(Arc<[u8]>),
    Object(usize),
    Native(&'static str),
}

impl Key {
    pub fn value(&self) -> Value {
        match self {
            Self::Boolean(value) => Value::Boolean(*value),
            Self::Number(value) => Value::Number(f64::from_bits(*value)),
            Self::Bytes(value) => Value::Bytes(value.clone()),
            Self::Object(_) | Self::Native(_) => Value::Nil,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    entries: Vec<(Key, Value)>,
}

impl Table {
    pub fn new() -> TableRef {
        Rc::new(RefCell::new(Self::default()))
    }

    pub fn get(&self, key: &Value) -> Value {
        let Some(key) = key.key() else {
            return Value::Nil;
        };
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == &key)
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Nil)
    }

    pub fn set(&mut self, key: Value, value: Value) -> bool {
        let Some(key) = key.key() else {
            return false;
        };
        if let Some(index) = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == &key)
        {
            if matches!(value, Value::Nil) {
                self.entries.remove(index);
            } else {
                self.entries[index].1 = value;
            }
        } else if !matches!(value, Value::Nil) {
            self.entries.push((key, value));
        }
        true
    }

    pub fn sequence_len(&self) -> usize {
        let mut length = 0;
        loop {
            let next = Value::Number((length + 1) as f64);
            if matches!(self.get(&next), Value::Nil) {
                return length;
            }
            length += 1;
        }
    }

    pub fn next(&self, previous: &Value) -> Option<(Value, Value)> {
        let index = if matches!(previous, Value::Nil) {
            0
        } else {
            let key = previous.key()?;
            self.entries.iter().position(|(entry, _)| entry == &key)? + 1
        };
        self.entries
            .get(index)
            .map(|(key, value)| (key.value(), value.clone()))
    }

    pub fn remove_sequence(&mut self, index: usize) -> Value {
        let length = self.sequence_len();
        if index == 0 || index > length {
            return Value::Nil;
        }
        let removed = self.get(&Value::Number(index as f64));
        for slot in index..length {
            let next = self.get(&Value::Number((slot + 1) as f64));
            self.set(Value::Number(slot as f64), next);
        }
        self.set(Value::Number(length as f64), Value::Nil);
        removed
    }
}

#[derive(Debug)]
pub struct Closure {
    pub prototype: Arc<Prototype>,
    pub upvalues: Vec<Cell>,
}
