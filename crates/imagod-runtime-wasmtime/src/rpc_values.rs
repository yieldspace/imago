use imago_protocol::{from_cbor, to_cbor};
use imagod_common::ImagodError;
use serde::{Deserialize, Serialize};
use wasmtime::component::{Val, types};

use crate::map_runtime_error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
enum RpcValue {
    Bool(bool),
    S8(i8),
    U8(u8),
    S16(i16),
    U16(u16),
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    Float32(f32),
    Float64(f64),
    Char(char),
    String(String),
    List(Vec<RpcValue>),
    Record(Vec<(String, RpcValue)>),
    Tuple(Vec<RpcValue>),
    Variant {
        name: String,
        value: Option<Box<RpcValue>>,
    },
    Enum(String),
    Option(Option<Box<RpcValue>>),
    Result {
        is_ok: bool,
        value: Option<Box<RpcValue>>,
    },
    Flags(Vec<String>),
}

pub(crate) fn encode_payload_values(
    values: &[Val],
    tys: &[types::Type],
) -> Result<Vec<u8>, ImagodError> {
    if values.len() != tys.len() {
        return Err(map_runtime_error(format!(
            "rpc payload value count mismatch: values={}, types={}",
            values.len(),
            tys.len()
        )));
    }
    let mut encoded = Vec::with_capacity(values.len());
    for (value, ty) in values.iter().zip(tys.iter()) {
        encoded.push(value_to_rpc(value, ty)?);
    }
    to_cbor(&encoded).map_err(|err| map_runtime_error(format!("rpc payload encode failed: {err}")))
}

pub(crate) fn decode_payload_values(
    payload: &[u8],
    tys: &[types::Type],
) -> Result<Vec<Val>, ImagodError> {
    let encoded: Vec<RpcValue> = from_cbor(payload)
        .map_err(|err| map_runtime_error(format!("rpc payload decode failed: {err}")))?;
    if encoded.len() != tys.len() {
        return Err(map_runtime_error(format!(
            "rpc payload value count mismatch: values={}, types={}",
            encoded.len(),
            tys.len()
        )));
    }
    let mut values = Vec::with_capacity(encoded.len());
    for (value, ty) in encoded.iter().zip(tys.iter()) {
        values.push(rpc_to_value(value, ty)?);
    }
    Ok(values)
}

pub(crate) fn placeholder_values(tys: &[types::Type]) -> Result<Vec<Val>, ImagodError> {
    let mut out = Vec::with_capacity(tys.len());
    for ty in tys {
        out.push(placeholder_for_type(ty)?);
    }
    Ok(out)
}

fn placeholder_for_type(ty: &types::Type) -> Result<Val, ImagodError> {
    Ok(match ty {
        types::Type::Bool => Val::Bool(false),
        types::Type::S8 => Val::S8(0),
        types::Type::U8 => Val::U8(0),
        types::Type::S16 => Val::S16(0),
        types::Type::U16 => Val::U16(0),
        types::Type::S32 => Val::S32(0),
        types::Type::U32 => Val::U32(0),
        types::Type::S64 => Val::S64(0),
        types::Type::U64 => Val::U64(0),
        types::Type::Float32 => Val::Float32(0.0),
        types::Type::Float64 => Val::Float64(0.0),
        types::Type::Char => Val::Char('\0'),
        types::Type::String => Val::String(String::new()),
        types::Type::List(_) => Val::List(Vec::new()),
        types::Type::Record(_) => Val::Record(Vec::new()),
        types::Type::Tuple(_) => Val::Tuple(Vec::new()),
        types::Type::Variant(variant_ty) => {
            let first_case = variant_ty
                .cases()
                .next()
                .ok_or_else(|| map_runtime_error("variant type has no cases".to_string()))?;
            Val::Variant(first_case.name.to_string(), None)
        }
        types::Type::Enum(enum_ty) => {
            let first_case = enum_ty
                .names()
                .next()
                .ok_or_else(|| map_runtime_error("enum type has no cases".to_string()))?;
            Val::Enum(first_case.to_string())
        }
        types::Type::Option(_) => Val::Option(None),
        types::Type::Result(_) => Val::Result(Ok(None)),
        types::Type::Flags(_) => Val::Flags(Vec::new()),
        types::Type::Own(_)
        | types::Type::Borrow(_)
        | types::Type::Future(_)
        | types::Type::Stream(_)
        | types::Type::ErrorContext => {
            return Err(map_runtime_error(
                "resource values are not supported in rpc payload".to_string(),
            ));
        }
    })
}

fn value_to_rpc(value: &Val, ty: &types::Type) -> Result<RpcValue, ImagodError> {
    Ok(match (value, ty) {
        (Val::Bool(v), types::Type::Bool) => RpcValue::Bool(*v),
        (Val::S8(v), types::Type::S8) => RpcValue::S8(*v),
        (Val::U8(v), types::Type::U8) => RpcValue::U8(*v),
        (Val::S16(v), types::Type::S16) => RpcValue::S16(*v),
        (Val::U16(v), types::Type::U16) => RpcValue::U16(*v),
        (Val::S32(v), types::Type::S32) => RpcValue::S32(*v),
        (Val::U32(v), types::Type::U32) => RpcValue::U32(*v),
        (Val::S64(v), types::Type::S64) => RpcValue::S64(*v),
        (Val::U64(v), types::Type::U64) => RpcValue::U64(*v),
        (Val::Float32(v), types::Type::Float32) => RpcValue::Float32(*v),
        (Val::Float64(v), types::Type::Float64) => RpcValue::Float64(*v),
        (Val::Char(v), types::Type::Char) => RpcValue::Char(*v),
        (Val::String(v), types::Type::String) => RpcValue::String(v.clone()),
        (Val::List(items), types::Type::List(list_ty)) => {
            let item_ty = list_ty.ty();
            let mut encoded = Vec::with_capacity(items.len());
            for item in items {
                encoded.push(value_to_rpc(item, &item_ty)?);
            }
            RpcValue::List(encoded)
        }
        (Val::Record(fields), types::Type::Record(record_ty)) => {
            let expected_fields = record_ty.fields().collect::<Vec<_>>();
            if fields.len() != expected_fields.len() {
                return Err(map_runtime_error(format!(
                    "record field count mismatch: values={}, types={}",
                    fields.len(),
                    expected_fields.len()
                )));
            }
            let mut encoded = Vec::with_capacity(fields.len());
            for ((actual_name, actual_value), expected_field) in
                fields.iter().zip(expected_fields.iter())
            {
                if actual_name != expected_field.name {
                    return Err(map_runtime_error(format!(
                        "record field name mismatch: expected='{}', got='{}'",
                        expected_field.name, actual_name
                    )));
                }
                encoded.push((
                    actual_name.clone(),
                    value_to_rpc(actual_value, &expected_field.ty)?,
                ));
            }
            RpcValue::Record(encoded)
        }
        (Val::Tuple(items), types::Type::Tuple(tuple_ty)) => {
            let expected_types = tuple_ty.types().collect::<Vec<_>>();
            if items.len() != expected_types.len() {
                return Err(map_runtime_error(format!(
                    "tuple arity mismatch: values={}, types={}",
                    items.len(),
                    expected_types.len()
                )));
            }
            let mut encoded = Vec::with_capacity(items.len());
            for (item, item_ty) in items.iter().zip(expected_types.iter()) {
                encoded.push(value_to_rpc(item, item_ty)?);
            }
            RpcValue::Tuple(encoded)
        }
        (Val::Variant(name, value), types::Type::Variant(variant_ty)) => {
            let case = variant_ty
                .cases()
                .find(|case| case.name == name)
                .ok_or_else(|| {
                    map_runtime_error(format!("variant case '{}' is not defined", name))
                })?;
            let encoded_value = match (value.as_deref(), case.ty) {
                (None, None) => None,
                (Some(actual), Some(case_ty)) => Some(Box::new(value_to_rpc(actual, &case_ty)?)),
                (None, Some(_)) => {
                    return Err(map_runtime_error(format!(
                        "variant '{}' requires payload",
                        name
                    )));
                }
                (Some(_), None) => {
                    return Err(map_runtime_error(format!(
                        "variant '{}' must not contain payload",
                        name
                    )));
                }
            };
            RpcValue::Variant {
                name: name.clone(),
                value: encoded_value,
            }
        }
        (Val::Enum(name), types::Type::Enum(enum_ty)) => {
            if !enum_ty.names().any(|candidate| candidate == name) {
                return Err(map_runtime_error(format!(
                    "enum case '{}' is not defined",
                    name
                )));
            }
            RpcValue::Enum(name.clone())
        }
        (Val::Option(value), types::Type::Option(option_ty)) => {
            let encoded = match value.as_deref() {
                Some(inner) => Some(Box::new(value_to_rpc(inner, &option_ty.ty())?)),
                None => None,
            };
            RpcValue::Option(encoded)
        }
        (Val::Result(value), types::Type::Result(result_ty)) => match value {
            Ok(inner) => {
                let ok_ty = result_ty.ok();
                let encoded = match (inner.as_deref(), ok_ty.as_ref()) {
                    (None, None) => None,
                    (Some(actual), Some(ok_ty)) => Some(Box::new(value_to_rpc(actual, ok_ty)?)),
                    (None, Some(_)) => {
                        return Err(map_runtime_error(
                            "result ok payload is missing".to_string(),
                        ));
                    }
                    (Some(_), None) => {
                        return Err(map_runtime_error(
                            "result ok payload is not allowed".to_string(),
                        ));
                    }
                };
                RpcValue::Result {
                    is_ok: true,
                    value: encoded,
                }
            }
            Err(inner) => {
                let err_ty = result_ty.err();
                let encoded = match (inner.as_deref(), err_ty.as_ref()) {
                    (None, None) => None,
                    (Some(actual), Some(err_ty)) => Some(Box::new(value_to_rpc(actual, err_ty)?)),
                    (None, Some(_)) => {
                        return Err(map_runtime_error(
                            "result err payload is missing".to_string(),
                        ));
                    }
                    (Some(_), None) => {
                        return Err(map_runtime_error(
                            "result err payload is not allowed".to_string(),
                        ));
                    }
                };
                RpcValue::Result {
                    is_ok: false,
                    value: encoded,
                }
            }
        },
        (Val::Flags(names), types::Type::Flags(flags_ty)) => {
            for name in names {
                if !flags_ty.names().any(|flag| flag == name) {
                    return Err(map_runtime_error(format!("flag '{}' is not defined", name)));
                }
            }
            RpcValue::Flags(names.clone())
        }
        (_, types::Type::Own(_) | types::Type::Borrow(_)) => {
            return Err(map_runtime_error(
                "resource values are not supported in rpc payload".to_string(),
            ));
        }
        _ => {
            return Err(map_runtime_error(
                "rpc payload type mismatch for component value".to_string(),
            ));
        }
    })
}

fn rpc_to_value(value: &RpcValue, ty: &types::Type) -> Result<Val, ImagodError> {
    Ok(match (value, ty) {
        (RpcValue::Bool(v), types::Type::Bool) => Val::Bool(*v),
        (RpcValue::S8(v), types::Type::S8) => Val::S8(*v),
        (RpcValue::U8(v), types::Type::U8) => Val::U8(*v),
        (RpcValue::S16(v), types::Type::S16) => Val::S16(*v),
        (RpcValue::U16(v), types::Type::U16) => Val::U16(*v),
        (RpcValue::S32(v), types::Type::S32) => Val::S32(*v),
        (RpcValue::U32(v), types::Type::U32) => Val::U32(*v),
        (RpcValue::S64(v), types::Type::S64) => Val::S64(*v),
        (RpcValue::U64(v), types::Type::U64) => Val::U64(*v),
        (RpcValue::Float32(v), types::Type::Float32) => Val::Float32(*v),
        (RpcValue::Float64(v), types::Type::Float64) => Val::Float64(*v),
        (RpcValue::Char(v), types::Type::Char) => Val::Char(*v),
        (RpcValue::String(v), types::Type::String) => Val::String(v.clone()),
        (RpcValue::List(items), types::Type::List(list_ty)) => {
            let item_ty = list_ty.ty();
            let mut decoded = Vec::with_capacity(items.len());
            for item in items {
                decoded.push(rpc_to_value(item, &item_ty)?);
            }
            Val::List(decoded)
        }
        (RpcValue::Record(fields), types::Type::Record(record_ty)) => {
            let expected_fields = record_ty.fields().collect::<Vec<_>>();
            if fields.len() != expected_fields.len() {
                return Err(map_runtime_error(format!(
                    "record field count mismatch: values={}, types={}",
                    fields.len(),
                    expected_fields.len()
                )));
            }
            let mut decoded = Vec::with_capacity(fields.len());
            for ((actual_name, actual_value), expected_field) in
                fields.iter().zip(expected_fields.iter())
            {
                if actual_name != expected_field.name {
                    return Err(map_runtime_error(format!(
                        "record field name mismatch: expected='{}', got='{}'",
                        expected_field.name, actual_name
                    )));
                }
                decoded.push((
                    actual_name.clone(),
                    rpc_to_value(actual_value, &expected_field.ty)?,
                ));
            }
            Val::Record(decoded)
        }
        (RpcValue::Tuple(items), types::Type::Tuple(tuple_ty)) => {
            let expected_types = tuple_ty.types().collect::<Vec<_>>();
            if items.len() != expected_types.len() {
                return Err(map_runtime_error(format!(
                    "tuple arity mismatch: values={}, types={}",
                    items.len(),
                    expected_types.len()
                )));
            }
            let mut decoded = Vec::with_capacity(items.len());
            for (item, item_ty) in items.iter().zip(expected_types.iter()) {
                decoded.push(rpc_to_value(item, item_ty)?);
            }
            Val::Tuple(decoded)
        }
        (RpcValue::Variant { name, value }, types::Type::Variant(variant_ty)) => {
            let case = variant_ty
                .cases()
                .find(|case| case.name == name)
                .ok_or_else(|| {
                    map_runtime_error(format!("variant case '{}' is not defined", name))
                })?;
            let decoded_value = match (value.as_deref(), case.ty) {
                (None, None) => None,
                (Some(encoded), Some(case_ty)) => Some(Box::new(rpc_to_value(encoded, &case_ty)?)),
                (None, Some(_)) => {
                    return Err(map_runtime_error(format!(
                        "variant '{}' requires payload",
                        name
                    )));
                }
                (Some(_), None) => {
                    return Err(map_runtime_error(format!(
                        "variant '{}' must not contain payload",
                        name
                    )));
                }
            };
            Val::Variant(name.clone(), decoded_value)
        }
        (RpcValue::Enum(name), types::Type::Enum(enum_ty)) => {
            if !enum_ty.names().any(|candidate| candidate == name) {
                return Err(map_runtime_error(format!(
                    "enum case '{}' is not defined",
                    name
                )));
            }
            Val::Enum(name.clone())
        }
        (RpcValue::Option(value), types::Type::Option(option_ty)) => {
            let decoded = match value.as_deref() {
                Some(inner) => Some(Box::new(rpc_to_value(inner, &option_ty.ty())?)),
                None => None,
            };
            Val::Option(decoded)
        }
        (RpcValue::Result { is_ok, value }, types::Type::Result(result_ty)) => {
            if *is_ok {
                let ok_ty = result_ty.ok();
                let decoded = match (value.as_deref(), ok_ty.as_ref()) {
                    (None, None) => None,
                    (Some(encoded), Some(ok_ty)) => Some(Box::new(rpc_to_value(encoded, ok_ty)?)),
                    (None, Some(_)) => {
                        return Err(map_runtime_error(
                            "result ok payload is missing".to_string(),
                        ));
                    }
                    (Some(_), None) => {
                        return Err(map_runtime_error(
                            "result ok payload is not allowed".to_string(),
                        ));
                    }
                };
                Val::Result(Ok(decoded))
            } else {
                let err_ty = result_ty.err();
                let decoded = match (value.as_deref(), err_ty.as_ref()) {
                    (None, None) => None,
                    (Some(encoded), Some(err_ty)) => Some(Box::new(rpc_to_value(encoded, err_ty)?)),
                    (None, Some(_)) => {
                        return Err(map_runtime_error(
                            "result err payload is missing".to_string(),
                        ));
                    }
                    (Some(_), None) => {
                        return Err(map_runtime_error(
                            "result err payload is not allowed".to_string(),
                        ));
                    }
                };
                Val::Result(Err(decoded))
            }
        }
        (RpcValue::Flags(names), types::Type::Flags(flags_ty)) => {
            for name in names {
                if !flags_ty.names().any(|flag| flag == name) {
                    return Err(map_runtime_error(format!("flag '{}' is not defined", name)));
                }
            }
            Val::Flags(names.clone())
        }
        (_, types::Type::Own(_) | types::Type::Borrow(_)) => {
            return Err(map_runtime_error(
                "resource values are not supported in rpc payload".to_string(),
            ));
        }
        _ => {
            return Err(map_runtime_error(
                "rpc payload type mismatch for component value".to_string(),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::component::types;

    #[test]
    fn roundtrip_primitives() {
        let values = vec![Val::Bool(true), Val::S32(-1), Val::String("x".to_string())];
        let tys = vec![types::Type::Bool, types::Type::S32, types::Type::String];
        let payload = encode_payload_values(&values, &tys).expect("encode should succeed");
        let decoded = decode_payload_values(&payload, &tys).expect("decode should succeed");
        assert_eq!(decoded, values);
    }
}
