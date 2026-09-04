use prost::{
    bytes::{Buf, BufMut},
    encoding::{DecodeContext, WireType},
    DecodeError, Message,
};

use crate::{
    descriptor::{
        FieldDescriptor, Kind, KindIndex, RawFieldView, MAP_ENTRY_KEY_NUMBER,
        MAP_ENTRY_VALUE_NUMBER,
    },
    DynamicMessage, MapKey, Value,
};

use super::{
    fields::{FieldDescriptorLike, ValueAndDescriptor},
    unknown::UnknownField,
};

/// The handle-free description surface the encode path needs (C1).
///
/// Implemented by the borrowed [`RawFieldView`] the set-field iterator
/// yields (no Arc refcount traffic, map entries resolved once per map
/// field) and by [`ExtensionDescriptor`] for the extension encode arm.
/// The decode/merge path and the serde/text-format serializers use
/// `FieldDescriptorLike`/their own serializers and never see this
/// trait.
pub(super) trait EncodeFieldDesc {
    fn number(&self) -> u32;
    fn supports_presence(&self) -> bool;
    fn is_default_value(&self, value: &Value) -> bool;
    fn kind_index(&self) -> KindIndex;
    fn is_group(&self) -> bool;
    fn is_list(&self) -> bool;
    fn is_map(&self) -> bool;
    fn is_packed(&self) -> bool;
    /// Key/value field views of a map field, resolved once per field.
    /// Every implementer provides the real resolution (a default of None
    /// would make the map arm panic for that descriptor kind).
    fn map_entry_fields(&self) -> Option<(RawFieldView<'_>, RawFieldView<'_>)>;
}

impl EncodeFieldDesc for RawFieldView<'_> {
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

    fn map_entry_fields(&self) -> Option<(RawFieldView<'_>, RawFieldView<'_>)> {
        RawFieldView::map_entry_fields(self)
    }
}

impl EncodeFieldDesc for crate::ExtensionDescriptor {
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
    fn map_entry_fields(&self) -> Option<(RawFieldView<'_>, RawFieldView<'_>)> {
        self.map_entry_views()
    }
}

impl Message for DynamicMessage {
    fn encode_raw(&self, buf: &mut impl BufMut)
    where
        Self: Sized,
    {
        for field in self.fields.iter_set(&self.desc) {
            match field {
                // iter() (serde/text-format) never feeds encode_raw;
                // iter_set yields View for set fields
                ValueAndDescriptor::Field(..) => unreachable!("Field is encode-only via iter_set"),
                ValueAndDescriptor::View(value, view) => value.encode_field(&view, buf),
                ValueAndDescriptor::Extension(value, extension_desc) => {
                    value.encode_field(&extension_desc, buf)
                }
                ValueAndDescriptor::Unknown(unknowns) => unknowns.encode_raw(buf),
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
        if let Some(field_desc) = self.desc.get_field(number) {
            self.get_field_mut(&field_desc)
                .merge_field(&field_desc, wire_type, buf, ctx)
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
        let mut len = 0;
        for field in self.fields.iter_set(&self.desc) {
            match field {
                // iter() (serde/text-format) never feeds encoded_len;
                // iter_set yields View for set fields
                ValueAndDescriptor::Field(..) => unreachable!("Field is encode-only via iter_set"),
                ValueAndDescriptor::View(value, view) => {
                    len += value.encoded_len(&view);
                }
                ValueAndDescriptor::Extension(value, extension_desc) => {
                    len += value.encoded_len(&extension_desc);
                }
                ValueAndDescriptor::Unknown(unknowns) => len += unknowns.encoded_len(),
            }
        }
        len
    }

    fn clear(&mut self) {
        self.fields.clear_all();
    }
}

impl Value {
    pub(super) fn encode_field<B>(&self, field_desc: &impl EncodeFieldDesc, buf: &mut B)
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
        field_desc: &impl FieldDescriptorLike,
        wire_type: WireType,
        buf: &mut B,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError>
    where
        B: Buf,
    {
        match (self, field_desc.kind()) {
            (Value::Bool(value), Kind::Bool) => {
                prost::encoding::bool::merge(wire_type, value, buf, ctx)
            }
            (Value::I32(value), Kind::Int32) => {
                prost::encoding::int32::merge(wire_type, value, buf, ctx)
            }
            (Value::I32(value), Kind::Sint32) => {
                prost::encoding::sint32::merge(wire_type, value, buf, ctx)
            }
            (Value::I32(value), Kind::Sfixed32) => {
                prost::encoding::sfixed32::merge(wire_type, value, buf, ctx)
            }
            (Value::I64(value), Kind::Int64) => {
                prost::encoding::int64::merge(wire_type, value, buf, ctx)
            }
            (Value::I64(value), Kind::Sint64) => {
                prost::encoding::sint64::merge(wire_type, value, buf, ctx)
            }
            (Value::I64(value), Kind::Sfixed64) => {
                prost::encoding::sfixed64::merge(wire_type, value, buf, ctx)
            }
            (Value::U32(value), Kind::Uint32) => {
                prost::encoding::uint32::merge(wire_type, value, buf, ctx)
            }
            (Value::U32(value), Kind::Fixed32) => {
                prost::encoding::fixed32::merge(wire_type, value, buf, ctx)
            }
            (Value::U64(value), Kind::Uint64) => {
                prost::encoding::uint64::merge(wire_type, value, buf, ctx)
            }
            (Value::U64(value), Kind::Fixed64) => {
                prost::encoding::fixed64::merge(wire_type, value, buf, ctx)
            }
            (Value::F32(value), Kind::Float) => {
                prost::encoding::float::merge(wire_type, value, buf, ctx)
            }
            (Value::F64(value), Kind::Double) => {
                prost::encoding::double::merge(wire_type, value, buf, ctx)
            }
            (Value::String(value), Kind::String) => {
                prost::encoding::string::merge(wire_type, value, buf, ctx)
            }
            (Value::Bytes(value), Kind::Bytes) => {
                prost::encoding::bytes::merge(wire_type, value, buf, ctx)
            }
            (Value::EnumNumber(value), Kind::Enum(_)) => {
                prost::encoding::int32::merge(wire_type, value, buf, ctx)
            }
            (Value::Message(message), Kind::Message(_)) => {
                if field_desc.is_group() {
                    prost::encoding::group::merge(field_desc.number(), wire_type, message, buf, ctx)
                } else {
                    prost::encoding::message::merge(wire_type, message, buf, ctx)
                }
            }
            (Value::List(values), field_kind) if field_desc.is_list() => {
                if wire_type == WireType::LengthDelimited && field_desc.is_packable() {
                    prost::encoding::merge_loop(values, buf, ctx, |values, buf, ctx| {
                        let mut value = Value::default_value(&field_kind);
                        value.merge_field(field_desc, field_kind.wire_type(), buf, ctx)?;
                        values.push(value);
                        Ok(())
                    })
                } else {
                    let mut value = Value::default_value(&field_kind);
                    value.merge_field(field_desc, wire_type, buf, ctx)?;
                    values.push(value);
                    Ok(())
                }
            }
            (Value::Map(values), Kind::Message(map_entry)) if field_desc.is_map() => {
                let key_desc = map_entry.get_field(MAP_ENTRY_KEY_NUMBER).unwrap();
                let value_desc = map_entry.get_field(MAP_ENTRY_VALUE_NUMBER).unwrap();

                let mut key = MapKey::default_value(&key_desc.kind());
                let mut value = Value::default_value_for_field(&value_desc);
                prost::encoding::merge_loop(
                    &mut (&mut key, &mut value),
                    buf,
                    ctx,
                    |(key, value), buf, ctx| {
                        let (number, wire_type) = prost::encoding::decode_key(buf)?;
                        match number {
                            MAP_ENTRY_KEY_NUMBER => key.merge_field(&key_desc, wire_type, buf, ctx),
                            MAP_ENTRY_VALUE_NUMBER => {
                                value.merge_field(&value_desc, wire_type, buf, ctx)
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

    pub(super) fn encoded_len(&self, field_desc: &impl EncodeFieldDesc) -> usize {
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
    fn encode_field<B>(&self, field_desc: &impl EncodeFieldDesc, buf: &mut B)
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
        field_desc: &FieldDescriptor,
        wire_type: WireType,
        buf: &mut B,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError>
    where
        B: Buf,
    {
        match (self, field_desc.kind()) {
            (MapKey::Bool(value), Kind::Bool) => {
                prost::encoding::bool::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I32(value), Kind::Int32) => {
                prost::encoding::int32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I32(value), Kind::Sint32) => {
                prost::encoding::sint32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I32(value), Kind::Sfixed32) => {
                prost::encoding::sfixed32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I64(value), Kind::Int64) => {
                prost::encoding::int64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I64(value), Kind::Sint64) => {
                prost::encoding::sint64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::I64(value), Kind::Sfixed64) => {
                prost::encoding::sfixed64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::U32(value), Kind::Uint32) => {
                prost::encoding::uint32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::U32(value), Kind::Fixed32) => {
                prost::encoding::fixed32::merge(wire_type, value, buf, ctx)
            }
            (MapKey::U64(value), Kind::Uint64) => {
                prost::encoding::uint64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::U64(value), Kind::Fixed64) => {
                prost::encoding::fixed64::merge(wire_type, value, buf, ctx)
            }
            (MapKey::String(value), Kind::String) => {
                prost::encoding::string::merge(wire_type, value, buf, ctx)
            }
            (value, ty) => {
                panic!("mismatch between DynamicMessage value {value:?} and type {ty:?}")
            }
        }
    }

    fn encoded_len(&self, field_desc: &impl EncodeFieldDesc) -> usize {
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
