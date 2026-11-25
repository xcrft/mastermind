//! JSON parsing that rejects a repeated object key at any depth.
//!
//! `serde_json` keeps the last of repeated keys, so two readers of the same
//! bytes could disagree about what they say. Signed artifacts, controller
//! records and client configuration are read through here instead.

use serde::de::{DeserializeOwned, DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::fmt;

/// How a number with a fraction or exponent is treated.
#[derive(Clone, Copy)]
pub(crate) enum Floats {
    /// Signed and canonical records carry integers only.
    Reject,
    /// Client configuration files may carry fractional values.
    Accept,
}

pub(crate) fn parse(bytes: &[u8], floats: Floats) -> Result<Value, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Strict(floats).deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

pub(crate) fn from_slice<T: DeserializeOwned>(
    bytes: &[u8],
    floats: Floats,
) -> Result<T, serde_json::Error> {
    serde_json::from_value(parse(bytes, floats)?)
}

#[derive(Clone, Copy)]
struct Strict(Floats);

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        match self.0 {
            Floats::Reject => Err(E::custom("floating-point numbers are forbidden")),
            Floats::Accept => serde_json::Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| E::custom("non-finite JSON number")),
        }
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_string()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        self.deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(self)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom(format!("duplicate key: {key}")));
            }
            values.insert(key, map.next_value_seed(self)?);
        }
        Ok(Value::Object(values))
    }
}
