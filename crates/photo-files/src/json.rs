use serde::Serialize;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::{
    fmt,
    io::{self, Read},
};

struct JsonValue {
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for JsonValue {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if self.depth > 32 {
            return Err(de::Error::custom("JSON exceeds nesting limit"));
        }
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for JsonValue {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON without duplicate object keys")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(JsonValue {
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate JSON key: {key}")));
            }
            values.insert(
                key,
                object.next_value_seed(JsonValue {
                    depth: self.depth + 1,
                })?,
            );
        }
        Ok(Value::Object(values))
    }
}

pub fn read_json(reader: impl Read, maximum_bytes: usize) -> io::Result<Value> {
    let maximum = u64::try_from(maximum_bytes)
        .ok()
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid JSON size bound"))?;
    let mut bytes = Vec::new();
    reader.take(maximum).read_to_end(&mut bytes)?;
    if bytes.len() > maximum_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "JSON exceeds byte limit",
        ));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    let value = JsonValue { depth: 0 }
        .deserialize(&mut deserializer)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    deserializer
        .end()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(value)
}

pub fn canonical_json(value: &impl Serialize) -> io::Result<Vec<u8>> {
    // serde_json's default Map is key-ordered; array order remains meaningful.
    let value = serde_json::to_value(value).map_err(io::Error::other)?;
    serde_json::to_vec(&value).map_err(io::Error::other)
}
