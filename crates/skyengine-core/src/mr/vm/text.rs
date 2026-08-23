use std::{collections::BTreeMap, sync::Arc};

use crate::{Error, ResourceLimits, Result};

use super::{Constant, MrProfile, Prototype};

const RK_BASE: usize = 250;
const MAX_REGISTER_COUNT: usize = 250;
const JUMP_BIAS: isize = 131_071;

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Identifier(Arc<[u8]>),
    Number(f64),
    String(Arc<[u8]>),
    Def,
    If,
    Then,
    Else,
    End,
    Nil,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    Dot,
    Comma,
    Assign,
    Equal,
    Concat,
    Separator,
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    offset: usize,
}

#[derive(Clone, Debug)]
enum Statement {
    Function {
        name: Arc<[u8]>,
        parameters: Vec<Arc<[u8]>>,
        body: Vec<Statement>,
    },
    If {
        condition: Expression,
        body: Vec<Statement>,
        else_body: Vec<Statement>,
    },
    Assign {
        target: Expression,
        value: Expression,
    },
    Expression(Expression),
}

#[derive(Clone, Debug)]
enum Expression {
    Nil,
    Number(f64),
    String(Arc<[u8]>),
    Name(Arc<[u8]>),
    Member(Box<Expression>, Arc<[u8]>),
    Call(Box<Expression>, Vec<Expression>),
    Concat(Box<Expression>, Box<Expression>),
    Equal(Box<Expression>, Box<Expression>),
    Table(Vec<TableField>),
}

#[derive(Clone, Debug)]
enum TableField {
    Named(Arc<[u8]>, Expression),
    Array(Expression),
}

pub(super) fn compile(source: &[u8], limits: &ResourceLimits) -> Result<Arc<Prototype>> {
    if source.len() > limits.max_mr_string_len {
        return Err(Error::ResourceLimit(format!(
            "remote MR source length {} exceeds {}",
            source.len(),
            limits.max_mr_string_len
        )));
    }
    let tokens = lex(source, limits.max_mr_items)?;
    let statements = Parser::new(tokens, limits.max_mr_depth).parse()?;
    let prototype = Compiler::new(limits, true, Vec::new(), &statements)?.finish(statements)?;
    let (prototypes, items) = prototype_budget(&prototype);
    if prototypes > limits.max_mr_prototypes {
        return Err(Error::ResourceLimit(format!(
            "remote MR source has {prototypes} prototypes; limit is {}",
            limits.max_mr_prototypes
        )));
    }
    if items > limits.max_mr_items {
        return Err(Error::ResourceLimit(format!(
            "remote MR source compiles to {items} items; limit is {}",
            limits.max_mr_items
        )));
    }
    Ok(prototype)
}

fn lex(source: &[u8], max_tokens: usize) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        let offset = cursor;
        let byte = source[cursor];
        let kind = match byte {
            b' ' | b'\t' | b'\r' => {
                cursor += 1;
                continue;
            }
            b'\n' | b';' => {
                cursor += 1;
                TokenKind::Separator
            }
            b'(' => {
                cursor += 1;
                TokenKind::LeftParen
            }
            b')' => {
                cursor += 1;
                TokenKind::RightParen
            }
            b'{' => {
                cursor += 1;
                TokenKind::LeftBrace
            }
            b'}' => {
                cursor += 1;
                TokenKind::RightBrace
            }
            b',' => {
                cursor += 1;
                TokenKind::Comma
            }
            b'=' if source.get(cursor + 1) == Some(&b'=') => {
                cursor += 2;
                TokenKind::Equal
            }
            b'=' => {
                cursor += 1;
                TokenKind::Assign
            }
            b'.' if source.get(cursor + 1) == Some(&b'.') => {
                cursor += 2;
                TokenKind::Concat
            }
            b'.' => {
                cursor += 1;
                TokenKind::Dot
            }
            b'-' if source.get(cursor + 1) == Some(&b'-') => {
                cursor += 2;
                while cursor < source.len() && source[cursor] != b'\n' {
                    cursor += 1;
                }
                continue;
            }
            b'0'..=b'9' => {
                cursor += 1;
                while source
                    .get(cursor)
                    .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
                {
                    cursor += 1;
                }
                let text = std::str::from_utf8(&source[offset..cursor])
                    .map_err(|_| text_error(offset, "number contains non-UTF-8 bytes"))?;
                TokenKind::Number(
                    text.parse()
                        .map_err(|_| text_error(offset, "invalid numeric literal"))?,
                )
            }
            b'\'' | b'"' => {
                let quote = byte;
                cursor += 1;
                let mut value = Vec::new();
                loop {
                    let Some(byte) = source.get(cursor).copied() else {
                        return Err(text_error(offset, "unterminated string literal"));
                    };
                    cursor += 1;
                    if byte == quote {
                        break;
                    }
                    if byte != b'\\' {
                        value.push(byte);
                        continue;
                    }
                    let Some(escaped) = source.get(cursor).copied() else {
                        return Err(text_error(offset, "unterminated string escape"));
                    };
                    cursor += 1;
                    if escaped.is_ascii_digit() {
                        let mut decimal = u16::from(escaped - b'0');
                        for _ in 0..2 {
                            let Some(next) = source.get(cursor).copied() else {
                                break;
                            };
                            if !next.is_ascii_digit() {
                                break;
                            }
                            cursor += 1;
                            decimal = decimal * 10 + u16::from(next - b'0');
                        }
                        let byte = u8::try_from(decimal).map_err(|_| {
                            text_error(cursor, "decimal string escape exceeds one byte")
                        })?;
                        value.push(byte);
                        continue;
                    }
                    value.push(match escaped {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'\\' => b'\\',
                        b'\'' => b'\'',
                        b'"' => b'"',
                        other => other,
                    });
                }
                TokenKind::String(value.into())
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                cursor += 1;
                while source
                    .get(cursor)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    cursor += 1;
                }
                match &source[offset..cursor] {
                    b"def" => TokenKind::Def,
                    b"if" => TokenKind::If,
                    b"then" => TokenKind::Then,
                    b"else" => TokenKind::Else,
                    b"end" => TokenKind::End,
                    b"nil" => TokenKind::Nil,
                    identifier => TokenKind::Identifier(Arc::from(identifier)),
                }
            }
            _ => {
                return Err(text_error(offset, format!("unsupported byte {byte:#04x}")));
            }
        };
        tokens.push(Token { kind, offset });
        if tokens.len() > max_tokens {
            return Err(Error::ResourceLimit(format!(
                "remote MR source has more than {max_tokens} tokens"
            )));
        }
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        offset: source.len(),
    });
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    max_depth: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>, max_depth: usize) -> Self {
        Self {
            tokens,
            cursor: 0,
            max_depth,
        }
    }

    fn parse(mut self) -> Result<Vec<Statement>> {
        let statements = self.block(0, false)?;
        self.expect(TokenKind::Eof)?;
        Ok(statements)
    }

    fn block(&mut self, depth: usize, stop_at_end: bool) -> Result<Vec<Statement>> {
        if depth >= self.max_depth {
            return Err(Error::ResourceLimit(format!(
                "remote MR source nesting exceeds {}",
                self.max_depth
            )));
        }
        let mut statements = Vec::new();
        self.separators();
        while !matches!(self.peek(), TokenKind::Eof)
            && !(stop_at_end && matches!(self.peek(), TokenKind::Else | TokenKind::End))
        {
            statements.push(self.statement(depth)?);
            self.separators();
        }
        Ok(statements)
    }

    fn statement(&mut self, depth: usize) -> Result<Statement> {
        if self.take(&TokenKind::Def) {
            let name = self.identifier()?;
            self.expect(TokenKind::LeftParen)?;
            let mut parameters = Vec::new();
            if !matches!(self.peek(), TokenKind::RightParen) {
                loop {
                    parameters.push(self.identifier()?);
                    if !self.take(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(TokenKind::RightParen)?;
            self.separators();
            let body = self.block(depth + 1, true)?;
            self.expect(TokenKind::End)?;
            return Ok(Statement::Function {
                name,
                parameters,
                body,
            });
        }
        if self.take(&TokenKind::If) {
            let condition = self.expression()?;
            self.expect(TokenKind::Then)?;
            self.separators();
            let body = self.block(depth + 1, true)?;
            let else_body = if self.take(&TokenKind::Else) {
                self.separators();
                self.block(depth + 1, true)?
            } else {
                Vec::new()
            };
            self.expect(TokenKind::End)?;
            return Ok(Statement::If {
                condition,
                body,
                else_body,
            });
        }
        let expression = self.expression()?;
        if self.take(&TokenKind::Assign) {
            if !matches!(expression, Expression::Name(_) | Expression::Member(_, _)) {
                return Err(self.error("assignment target must be a name or table member"));
            }
            return Ok(Statement::Assign {
                target: expression,
                value: self.expression()?,
            });
        }
        if !matches!(expression, Expression::Call(_, _)) {
            return Err(self.error("only function calls may be expression statements"));
        }
        Ok(Statement::Expression(expression))
    }

    fn expression(&mut self) -> Result<Expression> {
        let mut expression = self.concat()?;
        if self.take(&TokenKind::Equal) {
            expression = Expression::Equal(Box::new(expression), Box::new(self.concat()?));
        }
        Ok(expression)
    }

    fn concat(&mut self) -> Result<Expression> {
        let left = self.postfix()?;
        if self.take(&TokenKind::Concat) {
            return Ok(Expression::Concat(Box::new(left), Box::new(self.concat()?)));
        }
        Ok(left)
    }

    fn postfix(&mut self) -> Result<Expression> {
        let mut expression = self.primary()?;
        loop {
            if self.take(&TokenKind::Dot) {
                expression = Expression::Member(Box::new(expression), self.identifier()?);
                continue;
            }
            if self.take(&TokenKind::LeftParen) {
                let mut arguments = Vec::new();
                if !matches!(self.peek(), TokenKind::RightParen) {
                    loop {
                        arguments.push(self.expression()?);
                        if !self.take(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(TokenKind::RightParen)?;
                expression = Expression::Call(Box::new(expression), arguments);
                continue;
            }
            break;
        }
        Ok(expression)
    }

    fn primary(&mut self) -> Result<Expression> {
        let token = self.tokens[self.cursor].clone();
        self.cursor += 1;
        match token.kind {
            TokenKind::Nil => Ok(Expression::Nil),
            TokenKind::Number(number) => Ok(Expression::Number(number)),
            TokenKind::String(bytes) => Ok(Expression::String(bytes)),
            TokenKind::Identifier(name) => Ok(Expression::Name(name)),
            TokenKind::LeftParen => {
                let expression = self.expression()?;
                self.expect(TokenKind::RightParen)?;
                Ok(expression)
            }
            TokenKind::LeftBrace => self.table(),
            _ => Err(text_error(token.offset, "expected expression")),
        }
    }

    fn table(&mut self) -> Result<Expression> {
        let mut fields = Vec::new();
        self.separators();
        while !self.take(&TokenKind::RightBrace) {
            let field = match (
                self.peek(),
                self.tokens.get(self.cursor + 1).map(|token| &token.kind),
            ) {
                (TokenKind::Identifier(_), Some(TokenKind::Assign)) => {
                    let name = self.identifier()?;
                    self.expect(TokenKind::Assign)?;
                    TableField::Named(name, self.expression()?)
                }
                _ => TableField::Array(self.expression()?),
            };
            fields.push(field);
            self.separators();
            if self.take(&TokenKind::Comma) {
                self.separators();
                continue;
            }
            self.expect(TokenKind::RightBrace)?;
            break;
        }
        Ok(Expression::Table(fields))
    }

    fn identifier(&mut self) -> Result<Arc<[u8]>> {
        let token = self.tokens[self.cursor].clone();
        self.cursor += 1;
        match token.kind {
            TokenKind::Identifier(identifier) => Ok(identifier),
            _ => Err(text_error(token.offset, "expected identifier")),
        }
    }

    fn separators(&mut self) {
        while self.take(&TokenKind::Separator) {}
    }

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.cursor].kind
    }

    fn take(&mut self, expected: &TokenKind) -> bool {
        if self.peek() == expected {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: TokenKind) -> Result<()> {
        if self.take(&expected) {
            Ok(())
        } else {
            Err(self.error(format!("expected {expected:?}, got {:?}", self.peek())))
        }
    }

    fn error(&self, message: impl Into<String>) -> Error {
        text_error(self.tokens[self.cursor].offset, message)
    }
}

struct Compiler<'a> {
    limits: &'a ResourceLimits,
    top_level: bool,
    constants: Vec<Constant>,
    prototypes: Vec<Arc<Prototype>>,
    code: Vec<u32>,
    locals: BTreeMap<Arc<[u8]>, usize>,
    next_register: usize,
    max_register: usize,
    parameter_count: usize,
}

impl<'a> Compiler<'a> {
    fn new(
        limits: &'a ResourceLimits,
        top_level: bool,
        parameters: Vec<Arc<[u8]>>,
        statements: &[Statement],
    ) -> Result<Self> {
        if parameters.len() > MAX_REGISTER_COUNT {
            return Err(text_error(0, "too many function parameters"));
        }
        let mut locals = BTreeMap::new();
        for parameter in &parameters {
            let index = locals.len();
            if locals.insert(parameter.clone(), index).is_some() {
                return Err(text_error(0, "duplicate function parameter"));
            }
        }
        if !top_level {
            collect_locals(statements, &mut locals)?;
        }
        if locals.len() > MAX_REGISTER_COUNT {
            return Err(text_error(0, "too many local variables"));
        }
        let next_register = locals.len();
        Ok(Self {
            limits,
            top_level,
            constants: Vec::new(),
            prototypes: Vec::new(),
            code: Vec::new(),
            locals,
            next_register,
            max_register: next_register,
            parameter_count: parameters.len(),
        })
    }

    fn finish(mut self, statements: Vec<Statement>) -> Result<Arc<Prototype>> {
        for statement in &statements {
            self.statement(statement)?;
        }
        self.emit_abc(27, 0, 1, 0)?;
        let stack_size = self.max_register.max(1);
        Ok(Arc::new(Prototype {
            profile: MrProfile::V50,
            source: Some(Arc::from(&b"@network"[..])),
            line_defined: 0,
            upvalue_count: 0,
            parameter_count: self.parameter_count as u8,
            is_vararg: false,
            max_stack_size: stack_size as u8,
            line_info: vec![0; self.code.len()],
            locals: Vec::new(),
            upvalue_names: Vec::new(),
            constants: self.constants,
            prototypes: self.prototypes,
            code: self.code,
        }))
    }

    fn statement(&mut self, statement: &Statement) -> Result<()> {
        self.next_register = self.locals.len();
        match statement {
            Statement::Function {
                name,
                parameters,
                body,
            } => {
                if !self.top_level {
                    return Err(text_error(0, "nested function definitions are unsupported"));
                }
                let child = Compiler::new(self.limits, false, parameters.clone(), body)?
                    .finish(body.clone())?;
                let prototype = self.prototypes.len();
                self.prototypes.push(child);
                let register = self.allocate(1)?;
                self.emit_abx(34, register, prototype)?;
                self.assign_name(name, register)?;
            }
            Statement::If {
                condition,
                body,
                else_body,
            } => {
                let false_jump = self.condition_jump(condition)?;
                for statement in body {
                    self.statement(statement)?;
                }
                if else_body.is_empty() {
                    self.patch_jump(false_jump, self.code.len())?;
                } else {
                    let end_jump = self.emit_jump()?;
                    self.patch_jump(false_jump, self.code.len())?;
                    for statement in else_body {
                        self.statement(statement)?;
                    }
                    self.patch_jump(end_jump, self.code.len())?;
                }
            }
            Statement::Assign { target, value } => match target {
                Expression::Name(name) => {
                    let register = self.allocate(1)?;
                    self.expression_into(value, register)?;
                    self.assign_name(name, register)?;
                }
                Expression::Member(object, key) => {
                    let registers = self.allocate(2)?;
                    self.expression_into(object, registers)?;
                    self.expression_into(value, registers + 1)?;
                    let key = self.rk_bytes(key)?;
                    self.emit_abc(9, registers, key, registers + 1)?;
                }
                _ => unreachable!("parser validates assignment targets"),
            },
            Statement::Expression(expression) => {
                let Expression::Call(function, arguments) = expression else {
                    unreachable!("parser validates expression statements");
                };
                self.call(function, arguments, None)?;
            }
        }
        self.next_register = self.locals.len();
        Ok(())
    }

    fn condition_jump(&mut self, condition: &Expression) -> Result<usize> {
        match condition {
            Expression::Equal(left, right) => {
                let registers = self.allocate(2)?;
                self.expression_into(left, registers)?;
                self.expression_into(right, registers + 1)?;
                self.emit_abc(21, 0, registers, registers + 1)?;
            }
            other => {
                let register = self.allocate(1)?;
                self.expression_into(other, register)?;
                self.emit_abc(24, register, register, 0)?;
            }
        }
        self.emit_jump()
    }

    fn expression_into(&mut self, expression: &Expression, destination: usize) -> Result<()> {
        self.note_register(destination)?;
        match expression {
            Expression::Nil => {
                let constant = self.constant(Constant::Nil)?;
                self.emit_abx(1, destination, constant)?;
            }
            Expression::Number(number) => {
                let constant = self.constant(Constant::Number(*number))?;
                self.emit_abx(1, destination, constant)?;
            }
            Expression::String(bytes) => {
                let constant = self.constant(Constant::Bytes(bytes.clone()))?;
                self.emit_abx(1, destination, constant)?;
            }
            Expression::Name(name) => {
                if let Some(register) = self.locals.get(name).copied() {
                    self.emit_abc(0, destination, register, 0)?;
                } else {
                    let constant = self.constant(Constant::Bytes(name.clone()))?;
                    self.emit_abx(5, destination, constant)?;
                }
            }
            Expression::Member(object, key) => {
                let object_register = self.allocate(1)?;
                self.expression_into(object, object_register)?;
                let key = self.rk_bytes(key)?;
                self.emit_abc(6, destination, object_register, key)?;
            }
            Expression::Call(function, arguments) => {
                self.call(function, arguments, Some(destination))?;
            }
            Expression::Concat(left, right) => {
                let registers = self.allocate(2)?;
                self.expression_into(left, registers)?;
                self.expression_into(right, registers + 1)?;
                self.emit_abc(19, destination, registers, registers + 1)?;
            }
            Expression::Equal(_, _) => {
                return Err(text_error(
                    0,
                    "equality expressions are only supported as if conditions",
                ));
            }
            Expression::Table(fields) => {
                self.emit_abc(10, destination, 0, 0)?;
                let mut array_index = 0;
                for field in fields {
                    let checkpoint = self.next_register;
                    let value_register = self.allocate(1)?;
                    let (key, value) = match field {
                        TableField::Named(key, value) => (self.rk_bytes(key)?, value),
                        TableField::Array(value) => {
                            array_index += 1;
                            (self.rk_number(f64::from(array_index))?, value)
                        }
                    };
                    self.expression_into(value, value_register)?;
                    self.emit_abc(9, destination, key, value_register)?;
                    self.next_register = checkpoint;
                }
            }
        }
        Ok(())
    }

    fn call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        destination: Option<usize>,
    ) -> Result<()> {
        let base = self.allocate(arguments.len() + 1)?;
        self.expression_into(function, base)?;
        for (index, argument) in arguments.iter().enumerate() {
            self.expression_into(argument, base + 1 + index)?;
        }
        self.emit_abc(
            25,
            base,
            arguments.len() + 1,
            if destination.is_some() { 2 } else { 1 },
        )?;
        if let Some(destination) = destination {
            self.emit_abc(0, destination, base, 0)?;
        }
        Ok(())
    }

    fn assign_name(&mut self, name: &[u8], register: usize) -> Result<()> {
        if let Some(destination) = self.locals.get(name).copied() {
            self.emit_abc(0, destination, register, 0)
        } else {
            let constant = self.constant(Constant::Bytes(Arc::from(name)))?;
            self.emit_abx(7, register, constant)
        }
    }

    fn allocate(&mut self, count: usize) -> Result<usize> {
        let start = self.next_register;
        let end = start
            .checked_add(count)
            .ok_or_else(|| text_error(0, "register allocation overflow"))?;
        if end > MAX_REGISTER_COUNT {
            return Err(text_error(0, "remote MR source needs too many registers"));
        }
        self.next_register = end;
        self.max_register = self.max_register.max(end);
        Ok(start)
    }

    fn note_register(&mut self, register: usize) -> Result<()> {
        if register >= MAX_REGISTER_COUNT {
            return Err(text_error(0, "remote MR register is out of range"));
        }
        self.max_register = self.max_register.max(register + 1);
        Ok(())
    }

    fn constant(&mut self, constant: Constant) -> Result<usize> {
        if let Some(index) = self
            .constants
            .iter()
            .position(|existing| constants_equal(existing, &constant))
        {
            return Ok(index);
        }
        if self.constants.len() >= 262_144 {
            return Err(text_error(0, "remote MR source has too many constants"));
        }
        let index = self.constants.len();
        self.constants.push(constant);
        Ok(index)
    }

    fn rk_bytes(&mut self, bytes: &[u8]) -> Result<usize> {
        let constant = self.constant(Constant::Bytes(Arc::from(bytes)))?;
        let operand = RK_BASE + constant;
        if operand >= 512 {
            return Err(text_error(
                0,
                "remote MR source has too many constants for a table key",
            ));
        }
        Ok(operand)
    }

    fn rk_number(&mut self, number: f64) -> Result<usize> {
        let constant = self.constant(Constant::Number(number))?;
        let operand = RK_BASE + constant;
        if operand >= 512 {
            return Err(text_error(
                0,
                "remote MR source has too many constants for a table key",
            ));
        }
        Ok(operand)
    }

    fn emit_abc(&mut self, opcode: u8, a: usize, b: usize, c: usize) -> Result<()> {
        if a >= 256 || b >= 512 || c >= 512 {
            return Err(text_error(0, "MR instruction operand is out of range"));
        }
        self.code
            .push(u32::from(opcode) | ((c as u32) << 6) | ((b as u32) << 15) | ((a as u32) << 24));
        Ok(())
    }

    fn emit_abx(&mut self, opcode: u8, a: usize, bx: usize) -> Result<()> {
        if a >= 256 || bx >= 262_144 {
            return Err(text_error(0, "MR instruction operand is out of range"));
        }
        self.code
            .push(u32::from(opcode) | ((bx as u32) << 6) | ((a as u32) << 24));
        Ok(())
    }

    fn emit_jump(&mut self) -> Result<usize> {
        let index = self.code.len();
        self.emit_abx(20, 0, JUMP_BIAS as usize)?;
        Ok(index)
    }

    fn patch_jump(&mut self, index: usize, target: usize) -> Result<()> {
        let offset = target as isize - index as isize - 1;
        let encoded = JUMP_BIAS
            .checked_add(offset)
            .filter(|encoded| (0..262_144).contains(encoded))
            .ok_or_else(|| text_error(0, "remote MR jump is out of range"))?;
        self.code[index] = u32::from(20_u8) | ((encoded as u32) << 6);
        Ok(())
    }
}

fn collect_locals(statements: &[Statement], locals: &mut BTreeMap<Arc<[u8]>, usize>) -> Result<()> {
    for statement in statements {
        match statement {
            Statement::Assign {
                target: Expression::Name(name),
                ..
            } => {
                if !locals.contains_key(name) {
                    let index = locals.len();
                    locals.insert(name.clone(), index);
                }
            }
            Statement::If {
                body, else_body, ..
            } => {
                collect_locals(body, locals)?;
                collect_locals(else_body, locals)?;
            }
            Statement::Function { .. } => {
                return Err(text_error(0, "nested function definitions are unsupported"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn constants_equal(left: &Constant, right: &Constant) -> bool {
    match (left, right) {
        (Constant::Nil, Constant::Nil) => true,
        (Constant::Number(left), Constant::Number(right)) => left.to_bits() == right.to_bits(),
        (Constant::Bytes(left), Constant::Bytes(right)) => left == right,
        _ => false,
    }
}

fn prototype_budget(prototype: &Prototype) -> (usize, usize) {
    let mut prototypes: usize = 1;
    let mut items = prototype
        .code
        .len()
        .saturating_add(prototype.constants.len());
    for child in &prototype.prototypes {
        let (child_prototypes, child_items) = prototype_budget(child);
        prototypes = prototypes.saturating_add(child_prototypes);
        items = items.saturating_add(child_items);
    }
    (prototypes, items)
}

fn text_error(offset: usize, message: impl Into<String>) -> Error {
    Error::MrFault(format!(
        "remote MR source at byte {offset}: {}",
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_the_update_progress_callback() {
        let source = br#"print("code frame received")
def progress(data)
  p = tonumber(data)
  if p == nil then
    p = 0
  end
  if g_dialog then
    if g_dialog.update then
      g_dialog.update(g_dialog, nil, data .. "%", p)
    end
  end
  if win then
    if win.refresh then
      win.refresh()
    end
  end
end
cmd.progress = progress"#;
        let prototype = compile(source, &ResourceLimits::default()).unwrap();
        assert_eq!(prototype.prototypes.len(), 1);
        assert!(prototype.code.len() >= 4);
        assert!(prototype.prototypes[0].code.len() >= 20);
    }

    #[test]
    fn compiles_binary_file_update_with_failure_branch() {
        let source = br#"f = file.open("applist.mrp", 10)
if f then
  f.write(f, "\077\082\080\071\000")
  f.close(f)
  if recreateApplist then
    recreateApplist(1)
  else
    net.successexit()
  end
else
  net.fail()
end"#;
        let prototype = compile(source, &ResourceLimits::default()).unwrap();
        assert!(prototype.constants.iter().any(
            |constant| matches!(constant, Constant::Bytes(bytes) if bytes.as_ref() == b"MRPG\0")
        ));
        assert!(prototype.code.len() >= 20);
    }

    #[test]
    fn compiles_nested_application_list_tables() {
        let source = br#"listver = 1
ignore_all = nil
list = {
  {t = "APP", e = "gghjt", v = 1002, ic = 1},
  {t = "APP", e = "talkcat", v = 1040, ic = 2},
}"#;
        let prototype = compile(source, &ResourceLimits::default()).unwrap();
        assert!(prototype.code.len() >= 20);
        assert!(prototype.constants.iter().any(
            |constant| matches!(constant, Constant::Bytes(bytes) if bytes.as_ref() == b"talkcat")
        ));
    }

    #[test]
    fn rejects_decimal_string_escapes_larger_than_a_byte() {
        let error = compile(b"print(\"\\256\")", &ResourceLimits::default()).unwrap_err();
        assert!(error.to_string().contains("exceeds one byte"));
    }

    #[test]
    fn rejects_source_outside_the_supported_grammar() {
        let error = compile(b"while true do end", &ResourceLimits::default()).unwrap_err();
        assert!(error.to_string().contains("only function calls"));
    }
}
