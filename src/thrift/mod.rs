//! Thrift support: wire protocols, the LINE IDL schema and a dynamic value
//! model that is converted to and from the wire using that schema.

pub mod codec;
pub mod schema;
pub mod value;
pub mod wire;

pub use codec::{decode_raw, decode_struct, decode_value, encode_struct, encode_value};
pub use schema::{BaseType, EnumDef, FieldDef, Schema, StructDef, TypeRef};
pub use value::{Map, Value};
pub use wire::{read_message, read_struct, write_message, write_struct, Protocol, TMessage, TType, TValue};
