use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// A 64-bit architectural value encoded as exactly `0x` plus sixteen lowercase hex digits.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HexU64(pub u64);

impl HexU64 {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for HexU64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&format_args!("0x{:016x}", self.0))
    }
}

struct HexU64Visitor;

impl<'de> Visitor<'de> for HexU64Visitor {
    type Value = HexU64;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a 64-bit value encoded as 0x followed by sixteen lowercase hex digits")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let digits = value
            .strip_prefix("0x")
            .ok_or_else(|| E::custom("missing 0x prefix"))?;
        if digits.len() != 16
            || !digits
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(E::custom("expected exactly sixteen lowercase hex digits"));
        }
        u64::from_str_radix(digits, 16)
            .map(HexU64)
            .map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for HexU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(HexU64Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::HexU64;

    #[test]
    fn round_trip_preserves_all_bits_as_fixed_width_hex() {
        let value = HexU64(u64::MAX);
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, "\"0xffffffffffffffff\"");
        assert_eq!(serde_json::from_str::<HexU64>(&json).unwrap(), value);
    }

    #[test]
    fn rejects_numbers_uppercase_and_short_values() {
        assert!(serde_json::from_str::<HexU64>("18446744073709551615").is_err());
        assert!(serde_json::from_str::<HexU64>("\"0xFFFFFFFFFFFFFFFF\"").is_err());
        assert!(serde_json::from_str::<HexU64>("\"0x1\"").is_err());
    }
}
