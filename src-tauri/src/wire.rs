pub(crate) mod u64_decimal {
    use std::fmt;

    use serde::{de::Visitor, Deserializer, Serializer};

    const MAX_SAFE_LEGACY_INTEGER: u64 = 9_007_199_254_740_991;

    pub fn serialize<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DecimalOrLegacySafeInteger;

        impl<'de> Visitor<'de> for DecimalOrLegacySafeInteger {
            type Value = u64;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a canonical unsigned decimal string or legacy safe integer")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value.is_empty()
                    || (value.len() > 1 && value.starts_with('0'))
                    || !value.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(E::custom("invalid u64 decimal string"));
                }
                value
                    .parse::<u64>()
                    .map_err(|_| E::custom("u64 decimal string is out of range"))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value > MAX_SAFE_LEGACY_INTEGER {
                    return Err(E::custom(
                        "legacy JSON integer exceeds the safe integer range",
                    ));
                }
                Ok(value)
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let value =
                    u64::try_from(value).map_err(|_| E::custom("u64 cannot be negative"))?;
                self.visit_u64(value)
            }
        }

        deserializer.deserialize_any(DecimalOrLegacySafeInteger)
    }
}
