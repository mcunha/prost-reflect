use prost::{
    bytes::{Buf, BufMut},
    encoding::{DecodeContext, WireType},
    DecodeError, Message,
};

use crate::{
    descriptor::{
        Kind, KindIndex, MessageDescriptor, RawFieldView, MAP_ENTRY_KEY_NUMBER,
        MAP_ENTRY_VALUE_NUMBER,
    },
    DynamicMessage, MapKey, Value,
};

use super::{
    fields::{FieldDescriptorLike, ValueOrUnknown},
    unknown::UnknownField,
};

/// The handle-free description surface the wire codecs need (C1):
/// used by the encode dispatch AND the decode/merge dispatch, so
/// neither direction builds `FieldDescriptor` handles per occurrence.
///
/// Implemented by the borrowed [`RawFieldView`] the set-field iterator
/// yields (no Arc refcount traffic, map entries resolved once per map
/// field) and by [`ExtensionDescriptor`] for the extension arms.
/// Serializers use `FieldDescriptorLike`/their own serializers.
pub(super) trait WireFieldDesc {
    fn number(&self) -> u32;
    fn supports_presence(&self) -> bool;
    fn is_default_value(&self, value: &Value) -> bool;
    fn kind_index(&self) -> KindIndex;
    fn is_group(&self) -> bool;
    fn is_list(&self) -> bool;
    fn is_map(&self) -> bool;
    fn is_packed(&self) -> bool;
    fn is_packable(&self) -> bool;
    /// Wire type of this field's kind (decode element loops).
    fn wire_type(&self) -> prost::encoding::WireType {
        super::wire_type_for_kind_index(self.kind_index())
    }
    /// Message descriptor for message/group kinds (decode element
    /// defaults); None for scalar kinds.
    fn kind_message_descriptor(&self) -> Option<MessageDescriptor> {
        None
    }
    /// First-declared number of an enum-kind field (proto2 implicit
    /// default); None for non-enum kinds.
    fn enum_default_value(&self) -> Option<i32> {
        None
    }
    /// Key/value field views of a map field, resolved once per field.
    /// Every implementer provides the real resolution (a default of None
    /// would make the map arm panic for that descriptor kind).
    fn map_entry_fields(&self) -> Option<(RawFieldView<'_>, RawFieldView<'_>)>;
}

impl WireFieldDesc for RawFieldView<'_> {
    fn number(&self) -> u32 {
        RawFieldView::number(self)
    }

    fn supports_presence(&self) -> bool {
        RawFieldView::supports_presence(self)
    }

    fn is_default_value(&self, value: &Value) -> bool {
        super::is_default_for_field_parts(
            value,
            self.is_list,
            self.is_map,
            RawFieldView::kind_index(self),
            RawFieldView::declared_default(self),
        )
    }

    fn kind_index(&self) -> KindIndex {
        RawFieldView::kind_index(self)
    }

    fn is_group(&self) -> bool {
        self.is_group
    }

    fn is_list(&self) -> bool {
        self.is_list
    }

    fn is_map(&self) -> bool {
        self.is_map
    }

    fn is_packed(&self) -> bool {
        RawFieldView::is_packed(self)
    }

    fn is_packable(&self) -> bool {
        self.is_list && RawFieldView::kind_index(self).is_packable()
    }

    fn kind_message_descriptor(&self) -> Option<MessageDescriptor> {
        RawFieldView::message_descriptor(self)
    }

    fn enum_default_value(&self) -> Option<i32> {
        RawFieldView::enum_default(self)
    }

    fn map_entry_fields(&self) -> Option<(RawFieldView<'_>, RawFieldView<'_>)> {
        RawFieldView::map_entry_fields(self)
    }
}

impl WireFieldDesc for crate::ExtensionDescriptor {
    fn number(&self) -> u32 {
        self.number()
    }
    fn supports_presence(&self) -> bool {
        self.supports_presence()
    }
    fn is_default_value(&self, value: &Value) -> bool {
        value.is_default_for_extension(self)
    }
    fn kind_index(&self) -> KindIndex {
        self.kind_index_inner()
    }
    fn is_group(&self) -> bool {
        self.is_group()
    }
    fn is_list(&self) -> bool {
        self.is_list()
    }
    fn is_map(&self) -> bool {
        self.is_map()
    }
    fn is_packed(&self) -> bool {
        self.is_packed()
    }
    fn is_packable(&self) -> bool {
        // No inherent ExtensionDescriptor::is_packable: direct trait call
        // would recurse; list + packable kind is the definition.
        self.is_list() && self.kind_index_inner().is_packable()
    }
    fn kind_message_descriptor(&self) -> Option<MessageDescriptor> {
        match self.kind() {
            Kind::Message(desc) => Some(desc),
            _ => None,
        }
    }

    fn enum_default_value(&self) -> Option<i32> {
        match self.kind() {
            Kind::Enum(desc) => Some(desc.default_value().number()),
            _ => None,
        }
    }

    fn map_entry_fields(&self) -> Option<(RawFieldView<'_>, RawFieldView<'_>)> {
        self.map_entry_views()
    }
}

/// Default value for an absent field (decode insertion): containers,
/// declared default, then kind default (message/group need the
/// descriptor; enum needs the first-declared number).
fn rawfield_absent_value(view: &RawFieldView<'_>) -> Value {
    if view.is_list {
        Value::List(Vec::new())
    } else if view.is_map {
        Value::Map(Default::default())
    } else if let Some(declared) = view.declared_default() {
        declared.clone()
    } else {
        super::value_default_for_kind_index(
            view.kind_index(),
            view.enum_default(),
            view.message_descriptor(),
        )
    }
}

/// One resolved set-field entry of the encode plan, in slot (ascending
/// number) order - the sequence the old `iter_set` yielded, so wire
/// order is unchanged. `Field` entries carry the resolved field index;
/// no-presence fields at their default were dropped at plan build, so
/// neither pass re-runs the presence predicate.
#[derive(Clone, Copy)]
enum EncodePlanEntry {
    Field {
        slot: u32,
        index: u32,
    },
    /// Value with no message field of that number (extension) or unknown
    /// bytes; resolved per pass exactly as `iter_set` did.
    Other {
        slot: u32,
        number: u32,
    },
}

/// Stack-inline plan buffer: <=16 set fields (every corpus shape, typical
/// proxy messages) never allocate; larger messages spill to the heap.
/// The plan is per-call, so DynamicMessage stays 40 bytes with no
/// cache-invalidation machinery.
struct EncodePlan {
    len: usize,
    inline: [EncodePlanEntry; 16],
    heap: Vec<EncodePlanEntry>,
}

impl EncodePlan {
    const EMPTY: EncodePlanEntry = EncodePlanEntry::Field { slot: 0, index: 0 };

    fn new() -> Self {
        EncodePlan {
            len: 0,
            inline: [Self::EMPTY; 16],
            heap: Vec::new(),
        }
    }

    fn push(&mut self, entry: EncodePlanEntry) {
        if self.len < self.inline.len() {
            self.inline[self.len] = entry;
        } else {
            self.heap.push(entry);
        }
        self.len += 1;
    }

    fn get(&self, i: usize) -> &EncodePlanEntry {
        if i < self.inline.len() {
            &self.inline[i]
        } else {
            &self.heap[i - self.inline.len()]
        }
    }

    fn iter(&self) -> impl Iterator<Item = &EncodePlanEntry> {
        (0..self.len).map(move |i| self.get(i))
    }
}

impl DynamicMessage {
    /// Plan-pass encode: classify every set field once (field index +
    /// presence), then both the length sum and the write reuse the plan -
    /// prost's `encode_to_vec` previously re-resolved every field in each
    /// of its two passes.
    fn build_encode_plan(&self) -> EncodePlan {
        let mut plan = EncodePlan::new();
        for (slot, (number, value)) in self.fields.slots().enumerate() {
            match value {
                ValueOrUnknown::Taken => {}
                ValueOrUnknown::Unknown(_) => plan.push(EncodePlanEntry::Other {
                    slot: slot as u32,
                    number,
                }),
                ValueOrUnknown::Value(value) => {
                    if let Some(index) = self.desc.field_index_by_number(number) {
                        let view = self.desc.view_for_index(index);
                        // Same presence filter iter_set applied: a
                        // no-presence field at its default is not encoded.
                        let present = view.supports_presence()
                            || !super::is_default_for_field_parts(
                                value,
                                view.is_list,
                                view.is_map,
                                view.kind_index(),
                                view.declared_default(),
                            );
                        if present {
                            plan.push(EncodePlanEntry::Field {
                                slot: slot as u32,
                                index,
                            });
                        }
                    } else {
                        plan.push(EncodePlanEntry::Other {
                            slot: slot as u32,
                            number,
                        });
                    }
                }
            }
        }
        plan
    }
}

impl Message for DynamicMessage {
    fn encode_raw(&self, buf: &mut impl BufMut)
    where
        Self: Sized,
    {
        let plan = self.build_encode_plan();
        for entry in plan.iter() {
            match entry {
                EncodePlanEntry::Field { slot, index } => {
                    let view = self.desc.view_for_index(*index);
                    match self.fields.value_at(*slot as usize) {
                        ValueOrUnknown::Value(value) => value.encode_field(&view, buf),
                        ValueOrUnknown::Unknown(_) | ValueOrUnknown::Taken => unreachable!(),
                    }
                }
                EncodePlanEntry::Other { slot, number } => {
                    match self.fields.value_at(*slot as usize) {
                        ValueOrUnknown::Value(value) => match self.desc.get_extension(*number) {
                            Some(extension_desc) if extension_desc.has(value) => {
                                value.encode_field(&extension_desc, buf)
                            }
                            Some(_) => {}
                            None => panic!("no field found with number {number}"),
                        },
                        ValueOrUnknown::Unknown(unknowns) => unknowns.encode_raw(buf),
                        ValueOrUnknown::Taken => {}
                    }
                }
            }
        }
    }

    fn merge_field(
        &mut self,
        number: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError>
    where
        Self: Sized,
    {
        if let Some(view) = self.desc.field_view(number) {
            if let Some(siblings) = view.oneof_sibling_numbers(number) {
                for sibling in siblings {
                    self.fields.clear_by_number(sibling);
                }
            }
            // Lazy default: the slot usually already holds a Value (e.g.
            // every element of a repeated field after the first), and the
            // default must not be built per wire occurrence - for
            // message-kind fields that is a DynamicMessage::new per call.
            self.fields
                .decode_entry(number, || rawfield_absent_value(&view))
                .merge_field(&view, wire_type, buf, ctx)
        } else if let Some(extension_desc) = self.desc.get_extension(number) {
            self.get_extension_mut(&extension_desc).merge_field(
                &extension_desc,
                wire_type,
                buf,
                ctx,
            )
        } else {
            let field = UnknownField::decode_value(number, wire_type, buf, ctx)?;
            self.fields.add_unknown(number, field);
            Ok(())
        }
    }

    fn encoded_len(&self) -> usize {
        let plan = self.build_encode_plan();
        let mut len = 0;
        for entry in plan.iter() {
            match entry {
                EncodePlanEntry::Field { slot, index } => {
                    let view = self.desc.view_for_index(*index);
                    match self.fields.value_at(*slot as usize) {
                        ValueOrUnknown::Value(value) => len += value.encoded_len(&view),
                        ValueOrUnknown::Unknown(_) | ValueOrUnknown::Taken => unreachable!(),
                    }
                }
                EncodePlanEntry::Other { slot, number } => {
                    match self.fields.value_at(*slot as usize) {
                        ValueOrUnknown::Value(value) => match self.desc.get_extension(*number) {
                            Some(extension_desc) if extension_desc.has(value) => {
                                len += value.encoded_len(&extension_desc);
                            }
                            Some(_) => {}
                            None => panic!("no field found with number {number}"),
                        },
                        ValueOrUnknown::Unknown(unknowns) => len += unknowns.encoded_len(),
                        ValueOrUnknown::Taken => {}
                    }
                }
            }
        }
        len
    }

    /// Override prost's default (encoded_len + encode_raw as separate
    /// passes): classify once, size exactly, write once.
    fn encode_to_vec(&self) -> Vec<u8>
    where
        Self: Sized,
    {
        let plan = self.build_encode_plan();
        let mut len = 0;
        for entry in plan.iter() {
            match entry {
                EncodePlanEntry::Field { slot, index } => {
                    let view = self.desc.view_for_index(*index);
                    match self.fields.value_at(*slot as usize) {
                        ValueOrUnknown::Value(value) => len += value.encoded_len(&view),
                        ValueOrUnknown::Unknown(_) | ValueOrUnknown::Taken => unreachable!(),
                    }
                }
                EncodePlanEntry::Other { slot, number } => {
                    match self.fields.value_at(*slot as usize) {
                        ValueOrUnknown::Value(value) => match self.desc.get_extension(*number) {
                            Some(extension_desc) if extension_desc.has(value) => {
                                len += value.encoded_len(&extension_desc);
                            }
                            Some(_) => {}
                            None => panic!("no field found with number {number}"),
                        },
                        ValueOrUnknown::Unknown(unknowns) => len += unknowns.encoded_len(),
                        ValueOrUnknown::Taken => {}
                    }
                }
            }
        }
        let mut buf = Vec::with_capacity(len);
        for entry in plan.iter() {
            match entry {
                EncodePlanEntry::Field { slot, index } => {
                    let view = self.desc.view_for_index(*index);
                    match self.fields.value_at(*slot as usize) {
                        ValueOrUnknown::Value(value) => value.encode_field(&view, &mut buf),
                        ValueOrUnknown::Unknown(_) | ValueOrUnknown::Taken => unreachable!(),
                    }
                }
                EncodePlanEntry::Other { slot, number } => {
                    match self.fields.value_at(*slot as usize) {
                        ValueOrUnknown::Value(value) => match self.desc.get_extension(*number) {
                            Some(extension_desc) if extension_desc.has(value) => {
                                value.encode_field(&extension_desc, &mut buf)
                            }
                            Some(_) => {}
                            None => panic!("no field found with number {number}"),
                        },
                        ValueOrUnknown::Unknown(unknowns) => unknowns.encode_raw(&mut buf),
                        ValueOrUnknown::Taken => {}
                    }
                }
            }
        }
        buf
    }

    fn clear(&mut self) {
        self.fields.clear_all();
    }
}

impl Value {
    pub(super) fn encode_field<B>(&self, field_desc: &impl WireFieldDesc, buf: &mut B)
    where
        B: BufMut,
    {
        if !field_desc.supports_presence() && field_desc.is_default_value(self) {
            return;
        }

        let number = field_desc.number();
        match (self, field_desc.kind_index()) {
            (Value::Bool(value), KindIndex::Bool) => {
                prost::encoding::bool::encode(number, value, buf)
            }
            (Value::I32(value), KindIndex::Int32) => {
                prost::encoding::int32::encode(number, value, buf)
            }
            (Value::I32(value), KindIndex::Sint32) => {
                prost::encoding::sint32::encode(number, value, buf)
            }
            (Value::I32(value), KindIndex::Sfixed32) => {
                prost::encoding::sfixed32::encode(number, value, buf)
            }
            (Value::I64(value), KindIndex::Int64) => {
                prost::encoding::int64::encode(number, value, buf)
            }
            (Value::I64(value), KindIndex::Sint64) => {
                prost::encoding::sint64::encode(number, value, buf)
            }
            (Value::I64(value), KindIndex::Sfixed64) => {
                prost::encoding::sfixed64::encode(number, value, buf)
            }
            (Value::U32(value), KindIndex::Uint32) => {
                prost::encoding::uint32::encode(number, value, buf)
            }
            (Value::U32(value), KindIndex::Fixed32) => {
                prost::encoding::fixed32::encode(number, value, buf)
            }
            (Value::U64(value), KindIndex::Uint64) => {
                prost::encoding::uint64::encode(number, value, buf)
            }
            (Value::U64(value), KindIndex::Fixed64) => {
                prost::encoding::fixed64::encode(number, value, buf)
            }
            (Value::F32(value), KindIndex::Float) => {
                prost::encoding::float::encode(number, value, buf)
            }
            (Value::F64(value), KindIndex::Double) => {
                prost::encoding::double::encode(number, value, buf)
            }
            (Value::String(value), KindIndex::String) => {
                prost::encoding::string::encode(number, value, buf)
            }
            (Value::Bytes(value), KindIndex::Bytes) => {
                prost::encoding::bytes::encode(number, value, buf)
            }
            (Value::EnumNumber(value), KindIndex::Enum(_)) => {
                prost::encoding::int32::encode(number, value, buf)
            }
            (Value::Message(message), KindIndex::Message(_) | KindIndex::Group(_)) => {
                if field_desc.is_group() {
                    prost::encoding::group::encode(number, message, buf)
                } else {
                    prost::encoding::message::encode(number, message, buf)
                }
            }
            (Value::List(values), _) if field_desc.is_list() => {
                if field_desc.is_packed() {
                    match field_desc.kind_index() {
                        KindIndex::Enum(_) => encode_packed_list(
                            number,
                            values
                                .iter()
                                .map(|v| v.as_enum_number().expect("expected enum number")),
                            buf,
                            |v, b| prost::encoding::encode_varint(v as u64, b),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        KindIndex::Double => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_f64().expect("expected double")),
                            buf,
                            |v, b| b.put_f64_le(v),
                            |_| 8,
                        ),
                        KindIndex::Float => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_f32().expect("expected float")),
                            buf,
                            |v, b| b.put_f32_le(v),
                            |_| 4,
                        ),
                        KindIndex::Int32 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_i32().expect("expected i32")),
                            buf,
                            |v, b| prost::encoding::encode_varint(v as u64, b),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        KindIndex::Int64 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_i64().expect("expected i64")),
                            buf,
                            |v, b| prost::encoding::encode_varint(v as u64, b),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        KindIndex::Uint32 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_u32().expect("expected u32")),
                            buf,
                            |v, b| prost::encoding::encode_varint(v as u64, b),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        KindIndex::Uint64 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_u64().expect("expected u64")),
                            buf,
                            |v, b| prost::encoding::encode_varint(v, b),
                            prost::encoding::encoded_len_varint,
                        ),
                        KindIndex::Sint32 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_i32().expect("expected i32")),
                            buf,
                            |v, b| prost::encoding::encode_varint(from_sint32(v) as u64, b),
                            |v| prost::encoding::encoded_len_varint(from_sint32(v) as u64),
                        ),
                        KindIndex::Sint64 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_i64().expect("expected i64")),
                            buf,
                            |v, b| prost::encoding::encode_varint(from_sint64(v), b),
                            |v| prost::encoding::encoded_len_varint(from_sint64(v)),
                        ),
                        KindIndex::Fixed32 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_u32().expect("expected u32")),
                            buf,
                            |v, b| b.put_u32_le(v),
                            |_| 4,
                        ),
                        KindIndex::Fixed64 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_u64().expect("expected u64")),
                            buf,
                            |v, b| b.put_u64_le(v),
                            |_| 8,
                        ),
                        KindIndex::Sfixed32 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_i32().expect("expected i32")),
                            buf,
                            |v, b| b.put_i32_le(v),
                            |_| 4,
                        ),
                        KindIndex::Sfixed64 => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_i64().expect("expected i64")),
                            buf,
                            |v, b| b.put_i64_le(v),
                            |_| 8,
                        ),
                        KindIndex::Bool => encode_packed_list(
                            number,
                            values.iter().map(|v| v.as_bool().expect("expected bool")),
                            buf,
                            |v, b| prost::encoding::encode_varint(v as u64, b),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        _ => panic!("invalid type for packed field in DynamicMessage"),
                    }
                } else {
                    for value in values {
                        value.encode_field(field_desc, buf);
                    }
                }
            }
            (Value::Map(values), _) if field_desc.is_map() => {
                let (key_view, value_view) = field_desc
                    .map_entry_fields()
                    .expect("map field must have an entry key and value");

                for (key, value) in values {
                    let len = key.encoded_len(&key_view) + value.encoded_len(&value_view);

                    prost::encoding::encode_key(number, WireType::LengthDelimited, buf);
                    prost::encoding::encode_varint(len as u64, buf);

                    key.encode_field(&key_view, buf);
                    value.encode_field(&value_view, buf);
                }
            }
            (value, ty) => {
                panic!("mismatch between DynamicMessage value {value:?} and type {ty:?}")
            }
        }
    }

    pub(super) fn merge_field<B>(
        &mut self,
        field_desc: &impl WireFieldDesc,
        wire_type: WireType,
        buf: &mut B,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError>
    where
        B: Buf,
    {
        match (self, field_desc.kind_index()) {
            (Value::Bool(value), KindIndex::Bool) => {
                prost::encoding::bool::merge(wire_type, value, buf, ctx)
            }
            (Value::I32(value), KindIndex::Int32) => {
                prost::encoding::int32::merge(wire_type, value, buf, ctx)
            }
            (Value::I32(value), KindIndex::Sint32) => {
                prost::encoding::sint32::merge(wire_type, value, buf, ctx)
            }
            (Value::I32(value), KindIndex::Sfixed32) => {
                prost::encoding::sfixed32::merge(wire_type, value, buf, ctx)
            }
            (Value::I64(value), KindIndex::Int64) => {
                prost::encoding::int64::merge(wire_type, value, buf, ctx)
            }
            (Value::I64(value), KindIndex::Sint64) => {
                prost::encoding::sint64::merge(wire_type, value, buf, ctx)
            }
            (Value::I64(value), KindIndex::Sfixed64) => {
                prost::encoding::sfixed64::merge(wire_type, value, buf, ctx)
            }
            (Value::U32(value), KindIndex::Uint32) => {
                prost::encoding::uint32::merge(wire_type, value, buf, ctx)
            }
            (Value::U32(value), KindIndex::Fixed32) => {
                prost::encoding::fixed32::merge(wire_type, value, buf, ctx)
            }
            (Value::U64(value), KindIndex::Uint64) => {
                prost::encoding::uint64::merge(wire_type, value, buf, ctx)
            }
            (Value::U64(value), KindIndex::Fixed64) => {
                prost::encoding::fixed64::merge(wire_type, value, buf, ctx)
            }
            (Value::F32(value), KindIndex::Float) => {
                prost::encoding::float::merge(wire_type, value, buf, ctx)
            }
            (Value::F64(value), KindIndex::Double) => {
                prost::encoding::double::merge(wire_type, value, buf, ctx)
            }
            (Value::String(value), KindIndex::String) => {
                prost::encoding::string::merge(wire_type, value, buf, ctx)
            }
            (Value::Bytes(value), KindIndex::Bytes) => {
                prost::encoding::bytes::merge(wire_type, value, buf, ctx)
            }
            (Value::EnumNumber(value), KindIndex::Enum(_)) => {
                prost::encoding::int32::merge(wire_type, value, buf, ctx)
            }
            (Value::Message(message), KindIndex::Message(_) | KindIndex::Group(_)) => {
                if field_desc.is_group() {
                    prost::encoding::group::merge(field_desc.number(), wire_type, message, buf, ctx)
                } else {
                    prost::encoding::message::merge(wire_type, message, buf, ctx)
                }
            }
            (Value::List(values), _) if field_desc.is_list() => {
                if wire_type == WireType::LengthDelimited && field_desc.is_packable() {
                    prost::encoding::merge_loop(values, buf, ctx, |values, buf, ctx| {
                        let mut value = super::value_default_for_kind_index(
                            field_desc.kind_index(),
                            field_desc.enum_default_value(),
                            field_desc.kind_message_descriptor(),
                        );
                        value.merge_field(field_desc, field_desc.wire_type(), buf, ctx)?;
                        values.push(value);
                        Ok(())
                    })
                } else {
                    let mut value = super::value_default_for_kind_index(
                        field_desc.kind_index(),
                        field_desc.enum_default_value(),
                        field_desc.kind_message_descriptor(),
                    );
                    value.merge_field(field_desc, wire_type, buf, ctx)?;
                    values.push(value);
                    Ok(())
                }
            }
            (Value::Map(values), _) if field_desc.is_map() => {
                let (key_view, value_view) = field_desc
                    .map_entry_fields()
                    .expect("map field must have an entry key and value");

                let mut key = super::mapkey_default_for_kind_index(key_view.kind_index());
                let mut value = rawfield_absent_value(&value_view);
                prost::encoding::merge_loop(
                    &mut (&mut key, &mut value),
                    buf,
                    ctx,
                    |(key, value), buf, ctx| {
                        let (number, wire_type) = prost::encoding::decode_key(buf)?;
                        match number {
                            MAP_ENTRY_KEY_NUMBER => key.merge_field(&key_view, wire_type, buf, ctx),
                            MAP_ENTRY_VALUE_NUMBER => {
                                value.merge_field(&value_view, wire_type, buf, ctx)
                            }
                            _ => prost::encoding::skip_field(wire_type, number, buf, ctx),
                        }
                    },
                )?;
                values.insert(key, value);

                Ok(())
            }
            (value, ty) => {
                panic!("mismatch between DynamicMessage value {value:?} and type {ty:?}")
            }
        }
    }

    pub(super) fn encoded_len(&self, field_desc: &impl WireFieldDesc) -> usize {
        if !field_desc.supports_presence() && field_desc.is_default_value(self) {
            return 0;
        }

        let number = field_desc.number();
        match (self, field_desc.kind_index()) {
            (Value::Bool(value), KindIndex::Bool) => {
                prost::encoding::bool::encoded_len(number, value)
            }
            (Value::I32(value), KindIndex::Int32) => {
                prost::encoding::int32::encoded_len(number, value)
            }
            (Value::I32(value), KindIndex::Sint32) => {
                prost::encoding::sint32::encoded_len(number, value)
            }
            (Value::I32(value), KindIndex::Sfixed32) => {
                prost::encoding::sfixed32::encoded_len(number, value)
            }
            (Value::I64(value), KindIndex::Int64) => {
                prost::encoding::int64::encoded_len(number, value)
            }
            (Value::I64(value), KindIndex::Sint64) => {
                prost::encoding::sint64::encoded_len(number, value)
            }
            (Value::I64(value), KindIndex::Sfixed64) => {
                prost::encoding::sfixed64::encoded_len(number, value)
            }
            (Value::U32(value), KindIndex::Uint32) => {
                prost::encoding::uint32::encoded_len(number, value)
            }
            (Value::U32(value), KindIndex::Fixed32) => {
                prost::encoding::fixed32::encoded_len(number, value)
            }
            (Value::U64(value), KindIndex::Uint64) => {
                prost::encoding::uint64::encoded_len(number, value)
            }
            (Value::U64(value), KindIndex::Fixed64) => {
                prost::encoding::fixed64::encoded_len(number, value)
            }
            (Value::F32(value), KindIndex::Float) => {
                prost::encoding::float::encoded_len(number, value)
            }
            (Value::F64(value), KindIndex::Double) => {
                prost::encoding::double::encoded_len(number, value)
            }
            (Value::String(value), KindIndex::String) => {
                prost::encoding::string::encoded_len(number, value)
            }
            (Value::Bytes(value), KindIndex::Bytes) => {
                prost::encoding::bytes::encoded_len(number, value)
            }
            (Value::EnumNumber(value), KindIndex::Enum(_)) => {
                prost::encoding::int32::encoded_len(number, value)
            }
            (Value::Message(message), KindIndex::Message(_) | KindIndex::Group(_)) => {
                if field_desc.is_group() {
                    prost::encoding::group::encoded_len(number, message)
                } else {
                    prost::encoding::message::encoded_len(number, message)
                }
            }
            (Value::List(values), _) if field_desc.is_list() => {
                if field_desc.is_packed() {
                    match field_desc.kind_index() {
                        KindIndex::Enum(_) => packed_list_encoded_len(
                            number,
                            values
                                .iter()
                                .map(|v| v.as_enum_number().expect("expected enum number")),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        KindIndex::Double => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_f64().expect("expected double")),
                            |_| 8,
                        ),
                        KindIndex::Float => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_f32().expect("expected float")),
                            |_| 4,
                        ),
                        KindIndex::Int32 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_i32().expect("expected i32")),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        KindIndex::Int64 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_i64().expect("expected i64")),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        KindIndex::Uint32 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_u32().expect("expected u32")),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        KindIndex::Uint64 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_u64().expect("expected u64")),
                            prost::encoding::encoded_len_varint,
                        ),
                        KindIndex::Sint32 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_i32().expect("expected i32")),
                            |v| prost::encoding::encoded_len_varint(from_sint32(v) as u64),
                        ),
                        KindIndex::Sint64 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_i64().expect("expected i64")),
                            |v| prost::encoding::encoded_len_varint(from_sint64(v)),
                        ),
                        KindIndex::Fixed32 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_u32().expect("expected u32")),
                            |_| 4,
                        ),
                        KindIndex::Fixed64 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_u64().expect("expected u64")),
                            |_| 8,
                        ),
                        KindIndex::Sfixed32 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_i32().expect("expected i32")),
                            |_| 4,
                        ),
                        KindIndex::Sfixed64 => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_i64().expect("expected i64")),
                            |_| 8,
                        ),
                        KindIndex::Bool => packed_list_encoded_len(
                            number,
                            values.iter().map(|v| v.as_bool().expect("expected bool")),
                            |v| prost::encoding::encoded_len_varint(v as u64),
                        ),
                        _ => panic!("invalid type for packed field in DynamicMessage"),
                    }
                } else {
                    values
                        .iter()
                        .map(|value| value.encoded_len(field_desc))
                        .sum()
                }
            }
            (Value::Map(values), _) if field_desc.is_map() => {
                let (key_view, value_view) = field_desc
                    .map_entry_fields()
                    .expect("map field must have an entry key and value");

                let key_len = prost::encoding::key_len(number);
                values
                    .iter()
                    .map(|(key, value)| {
                        let len = key.encoded_len(&key_view) + value.encoded_len(&value_view);

                        key_len + prost::encoding::encoded_len_varint(len as u64) + len
                    })
                    .sum::<usize>()
            }
            (value, ty) => {
                panic!("mismatch between DynamicMessage value {value:?} and type {ty:?}")
            }
        }
    }
}

impl MapKey {
    fn encode_field<B>(&self, field_desc: &impl WireFieldDesc, buf: &mut B)
    where
        B: BufMut,
    {
        if !field_desc.supports_presence()
            && super::is_default_mapkey(self, field_desc.kind_index())
        {
            return;
        }

        let number = field_desc.number();
        match (self, field_desc.kind_index()) {
            (MapKey::Bool(value), KindIndex::Bool) => {
                prost::encoding::bool::encode(number, value, buf)
            }
            (MapKey::I32(value), KindIndex::Int32) => {
                prost::encoding::int32::encode(number, value, buf)
            }
            (MapKey::I32(value), KindIndex::Sint32) => {
                prost::encoding::sint32::encode(number, value, buf)
            }
            (MapKey::I32(value), KindIndex::Sfixed32) => {
                prost::encoding::sfixed32::encode(number, value, buf)
            }
            (MapKey::I64(value), KindIndex::Int64) => {
                prost::encoding::int64::encode(number, value, buf)
            }
            (MapKey::I64(value), KindIndex::Sint64) => {
                prost::encoding::sint64::encode(number, value, buf)
            }
            (MapKey::I64(value), KindIndex::Sfixed64) => {
                prost::encoding::sfixed64::encode(number, value, buf)
            }
            (MapKey::U32(value), KindIndex::Uint32) => {
                prost::encoding::uint32::encode(number, value, buf)
            }
            (MapKey::U32(value), KindIndex::Fixed32) => {
                prost::encoding::fixed32::encode(number, value, buf)
            }
            (MapKey::U64(value), KindIndex::Uint64) => {
                prost::encoding::uint64::encode(number, value, buf)
            }
            (MapKey::U64(value), KindIndex::Fixed64) => {
                prost::encoding::fixed64::encode(number, value, buf)
            }
            (MapKey::String(value), KindIndex::String) => {
                prost::encoding::string::encode(number, value, buf)
            }
            (value, ty) => {
                panic!("mismatch between DynamicMessage value {value:?} and type {ty:?}")
            }
        }
    }

    fn merge_field<B>(
        &mut self,
        field_desc: &impl WireFieldDesc,
        wire_type: WireType,
        buf: &mut B,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError>
    where
        B: Buf,
    {
        match (self, field_desc.kind_index()) {
            (MapKey::Bool(value), KindIndex::Bool) => {
                prost::encoding::bool::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I32(value), KindIndex::Int32) => {
                prost::encoding::int32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I32(value), KindIndex::Sint32) => {
                prost::encoding::sint32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I32(value), KindIndex::Sfixed32) => {
                prost::encoding::sfixed32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I64(value), KindIndex::Int64) => {
                prost::encoding::int64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I64(value), KindIndex::Sint64) => {
                prost::encoding::sint64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I64(value), KindIndex::Sfixed64) => {
                prost::encoding::sfixed64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::U32(value), KindIndex::Uint32) => {
                prost::encoding::uint32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::U32(value), KindIndex::Fixed32) => {
                prost::encoding::fixed32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::U64(value), KindIndex::Uint64) => {
                prost::encoding::uint64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::U64(value), KindIndex::Fixed64) => {
                prost::encoding::fixed64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::String(value), KindIndex::String) => {
                prost::encoding::string::merge(wire_type, value, buf, ctx)
            }
            (value, ty) => {
                panic!("mismatch between DynamicMessage value {value:?} and type {ty:?}")
            }
        }
    }

    fn encoded_len(&self, field_desc: &impl WireFieldDesc) -> usize {
        if !field_desc.supports_presence()
            && super::is_default_mapkey(self, field_desc.kind_index())
        {
            return 0;
        }

        let number = field_desc.number();
        match (self, field_desc.kind_index()) {
            (MapKey::Bool(value), KindIndex::Bool) => {
                prost::encoding::bool::encoded_len(number, value)
            }
            (MapKey::I32(value), KindIndex::Int32) => {
                prost::encoding::int32::encoded_len(number, value)
            }
            (MapKey::I32(value), KindIndex::Sint32) => {
                prost::encoding::sint32::encoded_len(number, value)
            }
            (MapKey::I32(value), KindIndex::Sfixed32) => {
                prost::encoding::sfixed32::encoded_len(number, value)
            }
            (MapKey::I64(value), KindIndex::Int64) => {
                prost::encoding::int64::encoded_len(number, value)
            }
            (MapKey::I64(value), KindIndex::Sint64) => {
                prost::encoding::sint64::encoded_len(number, value)
            }
            (MapKey::I64(value), KindIndex::Sfixed64) => {
                prost::encoding::sfixed64::encoded_len(number, value)
            }
            (MapKey::U32(value), KindIndex::Uint32) => {
                prost::encoding::uint32::encoded_len(number, value)
            }
            (MapKey::U32(value), KindIndex::Fixed32) => {
                prost::encoding::fixed32::encoded_len(number, value)
            }
            (MapKey::U64(value), KindIndex::Uint64) => {
                prost::encoding::uint64::encoded_len(number, value)
            }
            (MapKey::U64(value), KindIndex::Fixed64) => {
                prost::encoding::fixed64::encoded_len(number, value)
            }
            (MapKey::String(value), KindIndex::String) => {
                prost::encoding::string::encoded_len(number, value)
            }
            (value, ty) => {
                panic!("mismatch between DynamicMessage value {value:?} and type {ty:?}")
            }
        }
    }
}

fn encode_packed_list<T, I, B, E, L>(number: u32, iter: I, buf: &mut B, encode: E, encoded_len: L)
where
    I: IntoIterator<Item = T> + Clone,
    B: BufMut,
    E: Fn(T, &mut B),
    L: Fn(T) -> usize,
{
    prost::encoding::encode_key(number, WireType::LengthDelimited, buf);
    let len: usize = iter.clone().into_iter().map(encoded_len).sum();
    prost::encoding::encode_varint(len as u64, buf);

    for value in iter {
        encode(value, buf);
    }
}

fn packed_list_encoded_len<T, I, L>(number: u32, iter: I, encoded_len: L) -> usize
where
    I: IntoIterator<Item = T>,
    L: Fn(T) -> usize,
{
    let len: usize = iter.into_iter().map(encoded_len).sum();
    prost::encoding::key_len(number) + prost::encoding::encoded_len_varint(len as u64) + len
}

fn from_sint32(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}
// fn to_sint32(value: u32) -> i32 {
//     ((value >> 1) as i32) ^ (-((value & 1) as i32))
// }
fn from_sint64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}
// fn to_sint64(value: u64) -> i64 {
//     ((value >> 1) as i64) ^ (-((value & 1) as i64))
// }
