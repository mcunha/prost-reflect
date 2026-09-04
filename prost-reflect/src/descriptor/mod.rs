mod api;
mod build;
mod error;
mod global;
mod tag;
#[cfg(test)]
mod tests;
pub(crate) mod types;

pub use self::error::DescriptorError;
use self::types::{DescriptorProto, EnumDescriptorProto};

use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    fmt,
    ops::Range,
    sync::Arc,
};

use crate::{descriptor::types::FileDescriptorProto, Value};

pub(crate) const MAP_ENTRY_KEY_NUMBER: u32 = 1;
pub(crate) const MAP_ENTRY_VALUE_NUMBER: u32 = 2;

pub(crate) const RESERVED_MESSAGE_FIELD_NUMBERS: Range<i32> = 19_000..20_000;
pub(crate) const VALID_MESSAGE_FIELD_NUMBERS: Range<i32> = 1..536_870_912;

/// Cardinality determines whether a field is optional, required, or repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cardinality {
    /// The field appears zero or one times.
    Optional,
    /// The field appears exactly one time. This cardinality is invalid with Proto3.
    Required,
    /// The field appears zero or more times.
    Repeated,
}

/// The syntax of a proto file.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub enum Syntax {
    /// The `proto2` syntax.
    Proto2,
    /// The `proto3` syntax.
    Proto3,
}

/// The type of a protobuf message field.
#[derive(Clone, PartialEq, Eq)]
pub enum Kind {
    /// The protobuf `double` type.
    Double,
    /// The protobuf `float` type.
    Float,
    /// The protobuf `int32` type.
    Int32,
    /// The protobuf `int64` type.
    Int64,
    /// The protobuf `uint32` type.
    Uint32,
    /// The protobuf `uint64` type.
    Uint64,
    /// The protobuf `sint32` type.
    Sint32,
    /// The protobuf `sint64` type.
    Sint64,
    /// The protobuf `fixed32` type.
    Fixed32,
    /// The protobuf `fixed64` type.
    Fixed64,
    /// The protobuf `sfixed32` type.
    Sfixed32,
    /// The protobuf `sfixed64` type.
    Sfixed64,
    /// The protobuf `bool` type.
    Bool,
    /// The protobuf `string` type.
    String,
    /// The protobuf `bytes` type.
    Bytes,
    /// A protobuf message type.
    Message(MessageDescriptor),
    /// A protobuf enum type.
    Enum(EnumDescriptor),
}

#[derive(Copy, Clone)]
pub(crate) enum KindIndex {
    Double,
    Float,
    Int32,
    Int64,
    Uint32,
    Uint64,
    Sint32,
    Sint64,
    Fixed32,
    Fixed64,
    Sfixed32,
    Sfixed64,
    Bool,
    String,
    Bytes,
    Message(MessageIndex),
    Enum(EnumIndex),
    Group(MessageIndex),
}

type DescriptorIndex = u32;
type FileIndex = DescriptorIndex;

/// Number -> field-index lookup for one message: one Vec of (number,
/// index) pairs kept sorted by number. Built once at pool load with the
/// exact declared field count (one allocation per message, none during
/// resolution); binary-search lookup; iteration is ascending field
/// number, matching the previous BTreeMap semantics (C1's merge-walk
/// relies on it).
#[derive(Clone)]
struct FieldNumberIndex {
    entries: Vec<(u32, FieldIndex)>,
}

impl FieldNumberIndex {
    /// Build-time constructor: the declared field count is known when the
    /// message shell is created, so the Vec is pre-sized exactly and
    /// resolution inserts never reallocate.
    fn with_capacity(fields: usize) -> Self {
        FieldNumberIndex {
            entries: Vec::with_capacity(fields),
        }
    }

    fn insert(&mut self, number: u32, index: FieldIndex) -> Option<FieldIndex> {
        match self.entries.binary_search_by_key(&number, |e| e.0) {
            Ok(pos) => {
                let previous = self.entries[pos].1;
                self.entries[pos].1 = index;
                Some(previous)
            }
            Err(pos) => {
                self.entries.insert(pos, (number, index));
                None
            }
        }
    }

    fn get(&self, number: u32) -> Option<FieldIndex> {
        self.entries
            .binary_search_by_key(&number, |e| e.0)
            .ok()
            .map(|pos| self.entries[pos].1)
    }

    fn iter_indices(&self) -> impl ExactSizeIterator<Item = FieldIndex> + '_ {
        self.entries.iter().map(|(_, index)| *index)
    }
}

type ServiceIndex = DescriptorIndex;
type MethodIndex = DescriptorIndex;
type MessageIndex = DescriptorIndex;
type FieldIndex = DescriptorIndex;
type OneofIndex = DescriptorIndex;
type ExtensionIndex = DescriptorIndex;
type EnumIndex = DescriptorIndex;
type EnumValueIndex = DescriptorIndex;

/// A `DescriptorPool` is a collection of related descriptors. Typically it will be created from
/// a [`FileDescriptorSet`][prost_types::FileDescriptorSet] output by the protobuf compiler
/// (see [`DescriptorPool::from_file_descriptor_set`]) but it may also be built up by adding files individually.
///
/// Methods like [`MessageDescriptor::extensions`] will be scoped to just the files contained within the parent
/// `DescriptorPool`.
///
/// This type uses reference counting internally so it is cheap to clone. Modifying an instance of a
/// pool will not update any existing clones of the instance.
#[derive(Clone, Default)]
pub struct DescriptorPool {
    inner: Arc<DescriptorPoolInner>,
}

#[derive(Clone, Default)]
struct DescriptorPoolInner {
    names: HashMap<Box<str>, Definition>,
    file_names: HashMap<Box<str>, FileIndex>,
    files: Vec<FileDescriptorInner>,
    messages: Vec<MessageDescriptorInner>,
    enums: Vec<EnumDescriptorInner>,
    extensions: Vec<ExtensionDescriptorInner>,
    services: Vec<ServiceDescriptorInner>,
}

#[derive(Clone)]
struct Identity {
    file: FileIndex,
    path: Box<[i32]>,
    full_name: Box<str>,
    name_index: usize,
}

#[derive(Clone, Debug)]
struct Definition {
    file: FileIndex,
    path: Box<[i32]>,
    kind: DefinitionKind,
}

#[derive(Copy, Clone, Debug)]
enum DefinitionKind {
    Package,
    Message(MessageIndex),
    Field(MessageIndex),
    Oneof(MessageIndex),
    Service(ServiceIndex),
    Method(ServiceIndex),
    Enum(EnumIndex),
    EnumValue(EnumIndex),
    Extension(ExtensionIndex),
}

/// A single source file containing protobuf messages and services.
#[derive(Clone, PartialEq, Eq)]
pub struct FileDescriptor {
    pool: DescriptorPool,
    index: FileIndex,
}

#[derive(Clone)]
struct FileDescriptorInner {
    syntax: Syntax,
    raw: FileDescriptorProto,
    prost: prost_types::FileDescriptorProto,
    dependencies: Vec<FileIndex>,
    transitive_dependencies: HashSet<FileIndex>,
}

/// A protobuf message definition.
#[derive(Clone, PartialEq, Eq)]
pub struct MessageDescriptor {
    pool: DescriptorPool,
    index: MessageIndex,
}

#[derive(Clone)]
struct MessageDescriptorInner {
    id: Identity,
    parent: Option<MessageIndex>,
    extensions: Vec<ExtensionIndex>,
    fields: Vec<FieldDescriptorInner>,
    field_numbers: FieldNumberIndex,
    field_names: HashMap<Box<str>, FieldIndex>,
    field_json_names: HashMap<Box<str>, FieldIndex>,
    oneofs: Vec<OneofDescriptorInner>,
    /// Whether this is a synthetic map-entry message (cached at pool
    /// build from the raw DescriptorProto options; immutable after).
    map_entry: bool,
}

impl MessageDescriptorInner {
    fn map_entry_flag(&self) -> bool {
        self.map_entry
    }
}

/// A oneof field in a protobuf message.
#[derive(Clone, PartialEq, Eq)]
pub struct OneofDescriptor {
    message: MessageDescriptor,
    index: OneofIndex,
}

#[derive(Clone)]
struct OneofDescriptorInner {
    id: Identity,
    fields: Vec<FieldIndex>,
}

/// A protobuf message definition.
#[derive(Clone, PartialEq, Eq)]
pub struct FieldDescriptor {
    message: MessageDescriptor,
    index: FieldIndex,
}

#[derive(Clone)]
struct FieldDescriptorInner {
    id: Identity,
    number: u32,
    json_name: Box<str>,
    kind: KindIndex,
    oneof: Option<OneofIndex>,
    is_packed: bool,
    supports_presence: bool,
    cardinality: Cardinality,
    default: Option<Value>,
}

/// A borrowed, handle-free view of a resolved field (C1): carries the
/// field's inner data plus enough pool access to resolve map entries and
/// oneof siblings, with no Arc refcount traffic. Constructed per set
/// field per pass by the encode iterators and per wire record by the
/// decode/merge dispatch; neither direction constructs FieldDescriptor
/// handles.
#[derive(Clone, Copy)]
pub(crate) struct RawFieldView<'a> {
    inner: &'a FieldDescriptorInner,
    pool: &'a DescriptorPool,
    /// Containing message index; consulted only by
    /// `oneof_sibling_numbers` (for map-entry views `inner.oneof` is
    /// always None so this is never dereferenced).
    message: MessageIndex,
    pub(crate) is_list: bool,
    pub(crate) is_map: bool,
    pub(crate) is_group: bool,
}

impl<'a> RawFieldView<'a> {
    fn new(
        inner: &'a FieldDescriptorInner,
        pool: &'a DescriptorPool,
        message: MessageIndex,
        is_list: bool,
        is_map: bool,
        is_group: bool,
    ) -> Self {
        RawFieldView {
            inner,
            pool,
            message,
            is_list,
            is_map,
            is_group,
        }
    }

    /// Decode: sibling field numbers of the oneof this field belongs to,
    /// if any (for last-wins sibling clearing). Borrowed, no allocation
    /// on the decode path.
    pub(crate) fn oneof_sibling_numbers(
        &self,
        self_number: u32,
    ) -> Option<impl Iterator<Item = u32> + 'a> {
        let oneof = self.inner.oneof?;
        let message: &'a _ = &self.pool.inner.messages[self.message as usize];
        Some(
            message.oneofs[oneof as usize]
                .fields
                .iter()
                .filter_map(move |&index| {
                    let number = message.fields[index as usize].number;
                    (number != self_number).then_some(number)
                }),
        )
    }

    /// Decode: the first-declared number of an enum-kind field (proto2
    /// implicit default), matching `EnumDescriptor::default_value`.
    pub(crate) fn enum_default(&self) -> Option<i32> {
        match self.inner.kind {
            KindIndex::Enum(enum_) => Some(self.pool.inner.enums[enum_ as usize].values[0].number),
            _ => None,
        }
    }

    /// Decode: an owned message handle for this field's message kind
    /// (only where a default child message must be built).
    pub(crate) fn message_descriptor(&self) -> Option<MessageDescriptor> {
        match self.inner.kind {
            KindIndex::Message(message) | KindIndex::Group(message) => Some(MessageDescriptor {
                pool: self.pool.clone(),
                index: message,
            }),
            _ => None,
        }
    }

    pub(crate) fn number(&self) -> u32 {
        self.inner.number
    }

    pub(crate) fn kind_index(&self) -> KindIndex {
        self.inner.kind
    }

    pub(crate) fn is_packed(&self) -> bool {
        self.inner.is_packed
    }

    pub(crate) fn supports_presence(&self) -> bool {
        self.inner.supports_presence
    }

    /// The declared proto2 default, when the field declares one (C1
    /// default exclusion needs it even on presence-less hand-built pools).
    pub(crate) fn declared_default(&self) -> Option<&'a Value> {
        self.inner.default.as_ref()
    }

    /// The map entry message's key and value fields as borrowed views,
    /// resolved once per map field (not per map entry).
    pub(crate) fn map_entry_fields(&self) -> Option<(RawFieldView<'a>, RawFieldView<'a>)> {
        let index = match self.inner.kind {
            KindIndex::Message(message) if self.is_map => message,
            _ => return None,
        };
        let entry = &self.pool.inner.messages[index as usize];
        let key = entry.field_by_number(MAP_ENTRY_KEY_NUMBER)?;
        let value = entry.field_by_number(MAP_ENTRY_VALUE_NUMBER)?;
        Some((
            RawFieldView::new(key, self.pool, index, false, false, false),
            RawFieldView::new(value, self.pool, index, false, false, false),
        ))
    }
}

impl MessageDescriptorInner {
    pub(crate) fn field_by_number(&self, number: u32) -> Option<&FieldDescriptorInner> {
        self.field_numbers
            .get(number)
            .map(|index| &self.fields[index as usize])
    }
}

/// A protobuf extension field definition.
#[derive(Clone, PartialEq, Eq)]
pub struct ExtensionDescriptor {
    pool: DescriptorPool,
    index: ExtensionIndex,
}

#[derive(Clone)]
pub struct ExtensionDescriptorInner {
    id: Identity,
    parent: Option<MessageIndex>,
    number: u32,
    json_name: Box<str>,
    extendee: MessageIndex,
    kind: KindIndex,
    is_packed: bool,
    cardinality: Cardinality,
    default: Option<Value>,
}

/// A protobuf enum type.
#[derive(Clone, PartialEq, Eq)]
pub struct EnumDescriptor {
    pool: DescriptorPool,
    index: EnumIndex,
}

#[derive(Clone)]
struct EnumDescriptorInner {
    id: Identity,
    parent: Option<MessageIndex>,
    values: Vec<EnumValueDescriptorInner>,
    value_numbers: Vec<(i32, EnumValueIndex)>,
    value_names: HashMap<Box<str>, EnumValueIndex>,
    allow_alias: bool,
}

/// A value in a protobuf enum type.
#[derive(Clone, PartialEq, Eq)]
pub struct EnumValueDescriptor {
    parent: EnumDescriptor,
    index: EnumValueIndex,
}

#[derive(Clone)]
struct EnumValueDescriptorInner {
    id: Identity,
    number: i32,
}

/// A protobuf service definition.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceDescriptor {
    pool: DescriptorPool,
    index: ServiceIndex,
}

#[derive(Clone)]
struct ServiceDescriptorInner {
    id: Identity,
    methods: Vec<MethodDescriptorInner>,
}

/// A method definition for a [`ServiceDescriptor`].
#[derive(Clone, PartialEq, Eq)]
pub struct MethodDescriptor {
    service: ServiceDescriptor,
    index: MethodIndex,
}

#[derive(Clone)]
struct MethodDescriptorInner {
    id: Identity,
    input: MessageIndex,
    output: MessageIndex,
}

impl Identity {
    fn new(file: FileIndex, path: &[i32], full_name: &str, name: &str) -> Identity {
        debug_assert!(full_name.ends_with(name));
        let name_index = full_name.len() - name.len();
        debug_assert!(name_index == 0 || full_name.as_bytes()[name_index - 1] == b'.');
        Identity {
            file,
            path: path.into(),
            full_name: full_name.into(),
            name_index,
        }
    }

    fn full_name(&self) -> &str {
        &self.full_name
    }

    fn name(&self) -> &str {
        &self.full_name[self.name_index..]
    }
}

impl KindIndex {
    pub(crate) fn is_packable(&self) -> bool {
        match self {
            KindIndex::Double
            | KindIndex::Float
            | KindIndex::Int32
            | KindIndex::Int64
            | KindIndex::Uint32
            | KindIndex::Uint64
            | KindIndex::Sint32
            | KindIndex::Sint64
            | KindIndex::Fixed32
            | KindIndex::Fixed64
            | KindIndex::Sfixed32
            | KindIndex::Sfixed64
            | KindIndex::Bool
            | KindIndex::Enum(_) => true,
            KindIndex::String | KindIndex::Bytes | KindIndex::Message(_) | KindIndex::Group(_) => {
                false
            }
        }
    }

    fn is_message(&self) -> bool {
        matches!(self, KindIndex::Message(_) | KindIndex::Group(_))
    }
}

impl fmt::Debug for KindIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KindIndex::Double => write!(f, "double"),
            KindIndex::Float => write!(f, "float"),
            KindIndex::Int32 => write!(f, "int32"),
            KindIndex::Int64 => write!(f, "int64"),
            KindIndex::Uint32 => write!(f, "uint32"),
            KindIndex::Uint64 => write!(f, "uint64"),
            KindIndex::Sint32 => write!(f, "sint32"),
            KindIndex::Sint64 => write!(f, "sint64"),
            KindIndex::Fixed32 => write!(f, "fixed32"),
            KindIndex::Fixed64 => write!(f, "fixed64"),
            KindIndex::Sfixed32 => write!(f, "sfixed32"),
            KindIndex::Sfixed64 => write!(f, "sfixed64"),
            KindIndex::Bool => write!(f, "bool"),
            KindIndex::String => write!(f, "string"),
            KindIndex::Bytes => write!(f, "bytes"),
            KindIndex::Message(_) | KindIndex::Group(_) => write!(f, "message"),
            KindIndex::Enum(_) => write!(f, "enum"),
        }
    }
}

impl DefinitionKind {
    fn is_parent(&self) -> bool {
        match self {
            DefinitionKind::Package => true,
            DefinitionKind::Message(_) => true,
            DefinitionKind::Field(_) => false,
            DefinitionKind::Oneof(_) => false,
            DefinitionKind::Service(_) => true,
            DefinitionKind::Method(_) => false,
            DefinitionKind::Enum(_) => true,
            DefinitionKind::EnumValue(_) => false,
            DefinitionKind::Extension(_) => false,
        }
    }
}

impl DescriptorPoolInner {
    fn get_by_name(&self, name: &str) -> Option<&Definition> {
        let name = name.strip_prefix('.').unwrap_or(name);
        self.names.get(name)
    }
}

fn to_index(i: usize) -> DescriptorIndex {
    i.try_into().expect("index too large")
}

fn find_message_proto<'a>(file: &'a FileDescriptorProto, path: &[i32]) -> &'a DescriptorProto {
    debug_assert_ne!(path.len(), 0);
    debug_assert_eq!(path.len() % 2, 0);

    let mut message: Option<&'a types::DescriptorProto> = None;
    for part in path.chunks(2) {
        match part[0] {
            tag::file::MESSAGE_TYPE => message = Some(&file.message_type[part[1] as usize]),
            tag::message::NESTED_TYPE => {
                message = Some(&message.unwrap().nested_type[part[1] as usize])
            }
            _ => panic!("invalid message path"),
        }
    }

    message.unwrap()
}

fn find_enum_proto<'a>(file: &'a FileDescriptorProto, path: &[i32]) -> &'a EnumDescriptorProto {
    debug_assert_ne!(path.len(), 0);
    debug_assert_eq!(path.len() % 2, 0);
    if path.len() == 2 {
        debug_assert_eq!(path[0], tag::file::ENUM_TYPE);
        &file.enum_type[path[1] as usize]
    } else {
        let message = find_message_proto(file, &path[..path.len() - 2]);
        debug_assert_eq!(path[path.len() - 2], tag::message::ENUM_TYPE);
        &message.enum_type[path[path.len() - 1] as usize]
    }
}

#[test]
fn assert_descriptor_send_sync() {
    fn test_send_sync<T: Send + Sync>() {}

    test_send_sync::<DescriptorPool>();
    test_send_sync::<Kind>();
    test_send_sync::<DescriptorError>();
}
