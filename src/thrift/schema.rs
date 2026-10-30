//! Minimal Thrift IDL parser and the schema built from LINE's bundled
//! `line.thrift`.
//!
//! linejs ships a 40k-line generated table (`@evex/linejs-types/thrift`) for
//! renaming field ids to names, plus 10k lines of generated writers. Here the
//! IDL is embedded and parsed once at runtime instead; both directions of the
//! codec are driven by the resulting [`Schema`].

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::error::{Error, Result};

const LINE_THRIFT: &str = include_str!("../../resources/line.thrift");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseType {
    Bool,
    Byte,
    I16,
    I32,
    I64,
    Double,
    String,
    Binary,
    /// `_any` in LINE's IDL: unknown shape, decoded and encoded by inference.
    Any,
    Void,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Base(BaseType),
    List(Box<TypeRef>),
    Set(Box<TypeRef>),
    Map(Box<TypeRef>, Box<TypeRef>),
    /// A struct, exception, union or enum name.
    Named(String),
}

impl TypeRef {
    pub const ANY: TypeRef = TypeRef::Base(BaseType::Any);
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub id: i16,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, Default)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    by_id: HashMap<i16, usize>,
    by_name: HashMap<String, usize>,
}

impl StructDef {
    pub fn field_by_id(&self, id: i16) -> Option<&FieldDef> {
        self.by_id.get(&id).map(|&i| &self.fields[i])
    }
    pub fn field_by_name(&self, name: &str) -> Option<&FieldDef> {
        self.by_name.get(name).map(|&i| &self.fields[i])
    }
}

#[derive(Debug, Clone, Default)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<(String, i32)>,
    by_value: HashMap<i32, usize>,
    by_name: HashMap<String, usize>,
}

impl EnumDef {
    pub fn name_of(&self, value: i32) -> Option<&str> {
        self.by_value.get(&value).map(|&i| self.variants[i].0.as_str())
    }
    pub fn value_of(&self, name: &str) -> Option<i32> {
        self.by_name.get(name).map(|&i| self.variants[i].1)
    }
}

#[derive(Debug, Default)]
pub struct Schema {
    pub structs: HashMap<String, StructDef>,
    pub enums: HashMap<String, EnumDef>,
}

pub enum Named<'a> {
    Struct(&'a StructDef),
    Enum(&'a EnumDef),
}

impl Schema {
    /// The schema parsed from LINE's bundled IDL (parsed lazily, once).
    pub fn line() -> &'static Schema {
        static SCHEMA: OnceLock<Schema> = OnceLock::new();
        SCHEMA.get_or_init(|| Schema::parse(LINE_THRIFT).expect("bundled line.thrift must parse"))
    }

    pub fn get_struct(&self, name: &str) -> Option<&StructDef> {
        self.structs.get(name)
    }

    pub fn get_enum(&self, name: &str) -> Option<&EnumDef> {
        self.enums.get(name)
    }

    pub fn resolve(&self, name: &str) -> Option<Named<'_>> {
        if let Some(s) = self.structs.get(name) {
            return Some(Named::Struct(s));
        }
        self.enums.get(name).map(Named::Enum)
    }

    /// Parse Thrift IDL source. Later definitions with the same name replace
    /// earlier ones (LINE's IDL contains a few duplicates).
    pub fn parse(src: &str) -> Result<Schema> {
        let tokens = tokenize(src);
        let mut p = Parser { tokens, pos: 0 };
        let mut schema = Schema::default();
        while let Some(tok) = p.peek() {
            match tok {
                "struct" | "exception" | "union" => {
                    p.next();
                    let s = p.parse_struct()?;
                    schema.structs.insert(s.name.clone(), s);
                }
                "enum" => {
                    p.next();
                    let e = p.parse_enum()?;
                    schema.enums.insert(e.name.clone(), e);
                }
                "namespace" | "include" => {
                    p.next();
                    p.next();
                    if tok == "namespace" {
                        p.next();
                    }
                }
                "typedef" | "const" | "service" => {
                    return Err(Error::Thrift(format!("unsupported IDL construct `{tok}`")));
                }
                other => return Err(Error::Thrift(format!("unexpected IDL token `{other}`"))),
            }
        }
        Ok(schema)
    }
}

fn tokenize(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'/') || c == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if b"{}<>,;:=()".contains(&c) {
            out.push(&src[i..i + 1]);
            i += 1;
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !b"{}<>,;:=()".contains(&bytes[i]) {
                i += 1;
            }
            out.push(&src[start..i]);
        }
    }
    out
}

struct Parser<'a> {
    tokens: Vec<&'a str>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a str> {
        self.tokens.get(self.pos).copied()
    }
    fn next(&mut self) -> Option<&'a str> {
        let t = self.peek();
        self.pos += 1;
        t
    }
    fn expect(&mut self, want: &str) -> Result<()> {
        match self.next() {
            Some(t) if t == want => Ok(()),
            other => Err(Error::Thrift(format!("IDL: expected `{want}`, got {other:?}"))),
        }
    }
    fn ident(&mut self) -> Result<String> {
        self.next().map(str::to_string).ok_or_else(|| Error::Thrift("IDL: unexpected EOF".into()))
    }
    fn skip_sep(&mut self) {
        while matches!(self.peek(), Some(",") | Some(";")) {
            self.pos += 1;
        }
    }

    fn parse_struct(&mut self) -> Result<StructDef> {
        let name = self.ident()?;
        self.expect("{")?;
        let mut s = StructDef { name, ..Default::default() };
        loop {
            self.skip_sep();
            if self.peek() == Some("}") {
                self.next();
                break;
            }
            let id: i16 = self
                .ident()?
                .parse()
                .map_err(|_| Error::Thrift(format!("IDL: bad field id in {}", s.name)))?;
            self.expect(":")?;
            if matches!(self.peek(), Some("optional") | Some("required")) {
                self.next();
            }
            let ty = self.parse_type()?;
            let fname = self.ident()?;
            if self.peek() == Some("=") {
                // default value: skip one token
                self.next();
                self.next();
            }
            let idx = s.fields.len();
            s.by_id.insert(id, idx);
            s.by_name.insert(fname.clone(), idx);
            s.fields.push(FieldDef { id, name: fname, ty });
        }
        Ok(s)
    }

    fn parse_type(&mut self) -> Result<TypeRef> {
        let t = self.ident()?;
        Ok(match t.as_str() {
            "bool" => TypeRef::Base(BaseType::Bool),
            "byte" | "i8" => TypeRef::Base(BaseType::Byte),
            "i16" => TypeRef::Base(BaseType::I16),
            "i32" => TypeRef::Base(BaseType::I32),
            "i64" => TypeRef::Base(BaseType::I64),
            "double" => TypeRef::Base(BaseType::Double),
            "string" => TypeRef::Base(BaseType::String),
            "binary" => TypeRef::Base(BaseType::Binary),
            "_any" => TypeRef::Base(BaseType::Any),
            "void" => TypeRef::Base(BaseType::Void),
            "list" | "set" => {
                self.expect("<")?;
                let inner = self.parse_type()?;
                self.expect(">")?;
                if t == "list" {
                    TypeRef::List(Box::new(inner))
                } else {
                    TypeRef::Set(Box::new(inner))
                }
            }
            "map" => {
                self.expect("<")?;
                let k = self.parse_type()?;
                self.expect(",")?;
                let v = self.parse_type()?;
                self.expect(">")?;
                TypeRef::Map(Box::new(k), Box::new(v))
            }
            _ => TypeRef::Named(t),
        })
    }

    fn parse_enum(&mut self) -> Result<EnumDef> {
        let name = self.ident()?;
        self.expect("{")?;
        let mut e = EnumDef { name, ..Default::default() };
        let mut next_value = 0i32;
        loop {
            self.skip_sep();
            if self.peek() == Some("}") {
                self.next();
                break;
            }
            let vname = self.ident()?;
            let value = if self.peek() == Some("=") {
                self.next();
                let raw = self.ident()?;
                parse_int(&raw).ok_or_else(|| Error::Thrift(format!("IDL: bad enum value {raw}")))?
            } else {
                next_value
            };
            next_value = value.wrapping_add(1);
            let idx = e.variants.len();
            e.by_value.entry(value).or_insert(idx);
            e.by_name.insert(vname.clone(), idx);
            e.variants.push((vname, value));
        }
        Ok(e)
    }
}

fn parse_int(s: &str) -> Option<i32> {
    if let Some(hex) = s.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok().map(|v| v as i32)
    } else {
        s.parse::<i64>().ok().map(|v| v as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_schema_parses() {
        let s = Schema::line();
        assert!(s.structs.len() > 2000, "structs: {}", s.structs.len());
        let args = s.get_struct("sendMessage_args").unwrap();
        assert_eq!(args.field_by_name("message").unwrap().id, 2);
        let msg = s.get_struct("Message").unwrap();
        assert_eq!(msg.field_by_id(15).unwrap().name, "contentType");
        let ct = s.get_enum("ContentType").unwrap();
        assert_eq!(ct.value_of("NONE"), Some(0));
        assert!(s.get_struct("SquareService_sendMessage_result").is_some());
        assert!(matches!(
            msg.field_by_name("contentMetadata").unwrap().ty,
            TypeRef::Map(..)
        ));
    }

    #[test]
    fn enum_without_values() {
        let s = Schema::parse("enum E { A, B = 5, C }").unwrap();
        let e = s.get_enum("E").unwrap();
        assert_eq!(e.value_of("A"), Some(0));
        assert_eq!(e.value_of("C"), Some(6));
    }
}
