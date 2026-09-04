use std::{borrow::Cow, fmt, mem::replace};

use crate::{
    ExtensionDescriptor, FieldDescriptor, Kind, MessageDescriptor, OneofDescriptor, Value,
};

use super::{
    unknown::{UnknownField, UnknownFieldSet},
    Either,
};

pub(crate) trait FieldDescriptorLike: fmt::Debug {
    #[cfg(feature = "text-format")]
    fn text_name(&self) -> &str;
    fn number(&self) -> u32;
    fn default_value(&self) -> Value;
    fn is_default_value(&self, value: &Value) -> bool;
    fn is_valid(&self, value: &Value) -> bool;
    fn containing_oneof(&self) -> Option<OneofDescriptor>;
    fn supports_presence(&self) -> bool;
    fn kind(&self) -> Kind;
    fn is_list(&self) -> bool;
    fn is_map(&self) -> bool;
    fn has(&self, value: &Value) -> bool {
        self.supports_presence() || !self.is_default_value(value)
    }
}

/// A set of fields (known, extension, and unknown) in a dynamic message.
///
/// C4: stored as a `Vec` kept sorted by field number (the same ascending
/// order a `BTreeMap` yielded), so sparse set-field iteration stays in a
/// valid protobuf wire order while lookups are binary searches and
/// wire-ascending decode insertion into an empty set appends with
/// near-zero cost (mid-vector inserts shift O(n) per occurrence; see the
/// C4 risk note in the tier1 design doc). `Taken` is only ever transient,
/// mid-iteration (draining iterators replace entries in place).
#[derive(Default, Debug, Clone, PartialEq)]
pub(super) struct DynamicMessageFieldSet {
    fields: Vec<(u32, ValueOrUnknown)>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ValueOrUnknown {
    /// Used to implement draining iterators.
    Taken,
    /// A protobuf value with known field type.
    Value(Value),
    /// One or more unknown fields.
    Unknown(UnknownFieldSet),
}

pub(super) enum ValueAndDescriptor<'a> {
    Field(Cow<'a, Value>, FieldDescriptor),
    Extension(Cow<'a, Value>, ExtensionDescriptor),
    Unknown(&'a UnknownFieldSet),
}

impl DynamicMessageFieldSet {
    fn pos(&self, number: u32) -> Result<usize, usize> {
        self.fields
            .binary_search_by_key(&number, |&(field_number, _)| field_number)
    }

    fn get_value(&self, number: u32) -> Option<&Value> {
        match self.pos(number) {
            Ok(index) => match &self.fields[index].1 {
                ValueOrUnknown::Value(value) => Some(value),
                ValueOrUnknown::Unknown(_) | ValueOrUnknown::Taken => None,
            },
            Err(_) => None,
        }
    }

    pub(super) fn has(&self, desc: &impl FieldDescriptorLike) -> bool {
        self.get_value(desc.number())
            .map(|value| desc.has(value))
            .unwrap_or(false)
    }

    pub(super) fn get(&self, desc: &impl FieldDescriptorLike) -> Cow<'_, Value> {
        match self.get_value(desc.number()) {
            Some(value) => Cow::Borrowed(value),
            None => Cow::Owned(desc.default_value()),
        }
    }

    pub(super) fn get_mut(&mut self, desc: &impl FieldDescriptorLike) -> &mut Value {
        self.clear_oneof_fields(desc);
        let number = desc.number();
        // Lazy default: the common case (slot already holds a Value) must
        // not pay to build desc.default_value() - for message-kind fields
        // that is a DynamicMessage::new (two Arc RMWs on the pool line).
        self.entry_mut(number, || desc.default_value())
    }

    /// Decode: return the mutable slot for `number`, building the default
    /// (only when the slot is absent or holds non-value state) and
    /// inserting it. Oneof sibling clearing is the caller's job via
    /// `clear_by_number`.
    pub(super) fn decode_entry(
        &mut self,
        number: u32,
        build_default: impl FnOnce() -> Value,
    ) -> &mut Value {
        self.entry_mut(number, build_default)
    }

    /// Mutable slot for `number`, inserting a default built by
    /// `build_default` when the slot is absent or held by non-value state
    /// (the closure runs only on those paths). Keeps the Vec sorted
    /// (binary-search probe; inserts mid-vector on out-of-order numbers -
    /// an O(n) shift per occurrence, while wire-ascending decode appends
    /// amortized O(1); see the C4 risk note on descending-order input).
    fn entry_mut(&mut self, number: u32, build_default: impl FnOnce() -> Value) -> &mut Value {
        match self.pos(number) {
            Ok(index) => match &mut self.fields[index].1 {
                ValueOrUnknown::Value(value) => value,
                slot => {
                    *slot = ValueOrUnknown::Value(build_default());
                    slot.unwrap_value_mut()
                }
            },
            Err(index) => {
                self.fields
                    .insert(index, (number, ValueOrUnknown::Value(build_default())));
                match &mut self.fields[index].1 {
                    ValueOrUnknown::Value(value) => value,
                    ValueOrUnknown::Unknown(_) | ValueOrUnknown::Taken => unreachable!(),
                }
            }
        }
    }

    pub(super) fn clear_by_number(&mut self, number: u32) {
        if let Ok(index) = self.pos(number) {
            self.fields.remove(index);
        }
    }

    pub(super) fn set(&mut self, desc: &impl FieldDescriptorLike, value: Value) {
        debug_assert!(
            desc.is_valid(&value),
            "invalid value {value:?} for field {desc:?}",
        );

        self.clear_oneof_fields(desc);
        let number = desc.number();
        match self.pos(number) {
            Ok(index) => self.fields[index].1 = ValueOrUnknown::Value(value),
            Err(index) => self
                .fields
                .insert(index, (number, ValueOrUnknown::Value(value))),
        }
    }

    fn clear_oneof_fields(&mut self, desc: &impl FieldDescriptorLike) {
        if let Some(oneof_desc) = desc.containing_oneof() {
            for oneof_field in oneof_desc.fields() {
                if oneof_field.number() != desc.number() {
                    self.clear(&oneof_field);
                }
            }
        }
    }

    pub(crate) fn add_unknown(&mut self, number: u32, unknown: UnknownField) {
        match self.pos(number) {
            Ok(index) => match &mut self.fields[index].1 {
                ValueOrUnknown::Value(_) => {
                    panic!("expected no field to be found with number {number}")
                }
                value @ ValueOrUnknown::Taken => {
                    *value = ValueOrUnknown::Unknown(UnknownFieldSet::from_iter([unknown]))
                }
                ValueOrUnknown::Unknown(unknowns) => unknowns.insert(unknown),
            },
            Err(index) => {
                self.fields.insert(
                    index,
                    (
                        number,
                        ValueOrUnknown::Unknown(UnknownFieldSet::from_iter([unknown])),
                    ),
                );
            }
        }
    }

    pub(super) fn clear(&mut self, desc: &impl FieldDescriptorLike) {
        self.clear_by_number(desc.number());
    }

    pub(crate) fn take(&mut self, desc: &impl FieldDescriptorLike) -> Option<Value> {
        let number = desc.number();
        if let Ok(index) = self.pos(number) {
            let (_, entry) = self.fields.remove(index);
            if let ValueOrUnknown::Value(value) = entry {
                if desc.has(&value) {
                    return Some(value);
                }
            }
        }
        None
    }

    /// Iterates over the fields in the message.
    ///
    /// If `include_default` is `true`, fields with their default value will be included.
    /// If `index_order` is `true`, fields will be iterated in the order they were defined in the source code. Otherwise, they will be iterated in field number order.
    pub(crate) fn iter<'a>(
        &'a self,
        message: &'a MessageDescriptor,
        include_default: bool,
        index_order: bool,
    ) -> impl Iterator<Item = ValueAndDescriptor<'a>> + 'a {
        let field_descriptors = if index_order {
            Either::Left(message.fields_in_index_order())
        } else {
            Either::Right(message.fields())
        };

        let fields = field_descriptors
            .filter(move |f| {
                if include_default {
                    !f.supports_presence() || self.has(f)
                } else {
                    self.has(f)
                }
            })
            .map(|f| ValueAndDescriptor::Field(self.get(&f), f));

        let extensions_unknowns =
            self.fields
                .iter()
                .filter_map(move |(number, value)| match value {
                    ValueOrUnknown::Value(value) => {
                        if let Some(extension) = message.get_extension(*number) {
                            if extension.has(value) {
                                Some(ValueAndDescriptor::Extension(
                                    Cow::Borrowed(value),
                                    extension,
                                ))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    ValueOrUnknown::Unknown(unknown) => Some(ValueAndDescriptor::Unknown(unknown)),
                    ValueOrUnknown::Taken => None,
                });

        fields.chain(extensions_unknowns)
    }

    /// Value at a storage slot (encode plan loops index slots directly;
    /// slot identity is stable while the field set is not structurally
    /// mutated).
    pub(super) fn value_at(&self, slot: usize) -> &ValueOrUnknown {
        &self.fields[slot].1
    }

    /// (number, value) for every stored slot, in ascending number order.
    pub(super) fn slots(&self) -> impl Iterator<Item = (u32, &ValueOrUnknown)> + '_ {
        self.fields.iter().map(|(number, value)| (*number, value))
    }

    pub(crate) fn iter_fields<'a>(
        &'a self,
        message: &'a MessageDescriptor,
    ) -> impl Iterator<Item = (FieldDescriptor, &'a Value)> + 'a {
        self.fields.iter().filter_map(move |(number, value)| {
            let value = match value {
                ValueOrUnknown::Value(value) => value,
                _ => return None,
            };
            let field = match message.get_field(*number) {
                Some(field) => field,
                _ => return None,
            };
            if field.has(value) {
                Some((field, value))
            } else {
                None
            }
        })
    }

    pub(crate) fn iter_extensions<'a>(
        &'a self,
        message: &'a MessageDescriptor,
    ) -> impl Iterator<Item = (ExtensionDescriptor, &'a Value)> + 'a {
        self.fields.iter().filter_map(move |(number, value)| {
            let value = match value {
                ValueOrUnknown::Value(value) => value,
                _ => return None,
            };
            let field = match message.get_extension(*number) {
                Some(field) => field,
                _ => return None,
            };
            if field.has(value) {
                Some((field, value))
            } else {
                None
            }
        })
    }

    pub(super) fn iter_unknown(&self) -> impl Iterator<Item = &'_ UnknownField> {
        self.fields.iter().flat_map(move |(_, value)| match value {
            ValueOrUnknown::Taken | ValueOrUnknown::Value(_) => [].iter(),
            ValueOrUnknown::Unknown(unknowns) => unknowns.iter(),
        })
    }

    pub(crate) fn iter_fields_mut<'a>(
        &'a mut self,
        message: &'a MessageDescriptor,
    ) -> impl Iterator<Item = (FieldDescriptor, &'a mut Value)> + 'a {
        self.fields.iter_mut().filter_map(move |(number, value)| {
            let value = match value {
                ValueOrUnknown::Value(value) => value,
                _ => return None,
            };
            let field = match message.get_field(*number) {
                Some(field) => field,
                _ => return None,
            };
            if field.has(value) {
                Some((field, value))
            } else {
                None
            }
        })
    }

    pub(crate) fn iter_extensions_mut<'a>(
        &'a mut self,
        message: &'a MessageDescriptor,
    ) -> impl Iterator<Item = (ExtensionDescriptor, &'a mut Value)> + 'a {
        self.fields.iter_mut().filter_map(move |(number, value)| {
            let value = match value {
                ValueOrUnknown::Value(value) => value,
                _ => return None,
            };
            let field = match message.get_extension(*number) {
                Some(field) => field,
                _ => return None,
            };
            if field.has(value) {
                Some((field, value))
            } else {
                None
            }
        })
    }

    pub(crate) fn take_fields<'a>(
        &'a mut self,
        message: &'a MessageDescriptor,
    ) -> impl Iterator<Item = (FieldDescriptor, Value)> + 'a {
        self.fields
            .iter_mut()
            .filter_map(move |(number, value_or_unknown)| {
                let value = match value_or_unknown {
                    ValueOrUnknown::Value(value) => value,
                    _ => return None,
                };
                let field = match message.get_field(*number) {
                    Some(field) => field,
                    _ => return None,
                };
                if field.has(value) {
                    Some((
                        field,
                        replace(value_or_unknown, ValueOrUnknown::Taken).unwrap_value(),
                    ))
                } else {
                    None
                }
            })
    }

    pub(crate) fn take_extensions<'a>(
        &'a mut self,
        message: &'a MessageDescriptor,
    ) -> impl Iterator<Item = (ExtensionDescriptor, Value)> + 'a {
        self.fields
            .iter_mut()
            .filter_map(move |(number, value_or_unknown)| {
                let value = match value_or_unknown {
                    ValueOrUnknown::Value(value) => value,
                    _ => return None,
                };
                let field = match message.get_extension(*number) {
                    Some(field) => field,
                    _ => return None,
                };
                if field.has(value) {
                    Some((
                        field,
                        replace(value_or_unknown, ValueOrUnknown::Taken).unwrap_value(),
                    ))
                } else {
                    None
                }
            })
    }

    pub(crate) fn take_unknown(&mut self) -> impl Iterator<Item = UnknownField> + '_ {
        self.fields
            .iter_mut()
            .flat_map(move |(_, value_or_unknown)| match value_or_unknown {
                ValueOrUnknown::Unknown(_) => replace(value_or_unknown, ValueOrUnknown::Taken)
                    .unwrap_unknown()
                    .into_iter(),
                _ => vec![].into_iter(),
            })
    }

    pub(super) fn clear_all(&mut self) {
        self.fields.clear();
    }
}
impl ValueOrUnknown {
    fn unwrap_value_mut(&mut self) -> &mut Value {
        match self {
            ValueOrUnknown::Value(value) => value,
            ValueOrUnknown::Unknown(_) | ValueOrUnknown::Taken => unreachable!(),
        }
    }

    fn unwrap_value(self) -> Value {
        match self {
            ValueOrUnknown::Value(value) => value,
            ValueOrUnknown::Unknown(_) | ValueOrUnknown::Taken => unreachable!(),
        }
    }

    fn unwrap_unknown(self) -> UnknownFieldSet {
        match self {
            ValueOrUnknown::Unknown(unknowns) => unknowns,
            ValueOrUnknown::Value(_) | ValueOrUnknown::Taken => unreachable!(),
        }
    }
}

impl FieldDescriptorLike for FieldDescriptor {
    #[cfg(feature = "text-format")]
    fn text_name(&self) -> &str {
        self.name()
    }

    fn number(&self) -> u32 {
        self.number()
    }

    fn default_value(&self) -> Value {
        Value::default_value_for_field(self)
    }

    fn is_default_value(&self, value: &Value) -> bool {
        value.is_default_for_field(self)
    }

    fn is_valid(&self, value: &Value) -> bool {
        value.is_valid_for_field(self)
    }

    fn containing_oneof(&self) -> Option<OneofDescriptor> {
        self.containing_oneof()
    }

    fn supports_presence(&self) -> bool {
        self.supports_presence()
    }

    fn kind(&self) -> Kind {
        self.kind()
    }

    fn is_list(&self) -> bool {
        self.is_list()
    }

    fn is_map(&self) -> bool {
        self.is_map()
    }
}

impl FieldDescriptorLike for ExtensionDescriptor {
    #[cfg(feature = "text-format")]
    fn text_name(&self) -> &str {
        self.json_name()
    }

    fn number(&self) -> u32 {
        self.number()
    }

    fn default_value(&self) -> Value {
        Value::default_value_for_extension(self)
    }

    fn is_default_value(&self, value: &Value) -> bool {
        value.is_default_for_extension(self)
    }

    fn is_valid(&self, value: &Value) -> bool {
        value.is_valid_for_extension(self)
    }

    fn containing_oneof(&self) -> Option<OneofDescriptor> {
        None
    }

    fn supports_presence(&self) -> bool {
        self.supports_presence()
    }

    fn kind(&self) -> Kind {
        self.kind()
    }

    fn is_list(&self) -> bool {
        self.is_list()
    }

    fn is_map(&self) -> bool {
        self.is_map()
    }
}
