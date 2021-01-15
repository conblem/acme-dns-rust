use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt::Formatter;

#[derive(Debug, Copy, Clone)]
pub enum ProxyProtocol {
    Enabled,
    Disabled,
}

impl<'de> Deserialize<'de> for ProxyProtocol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match bool::deserialize(deserializer)? {
            true => Ok(ProxyProtocol::Enabled),
            false => Ok(ProxyProtocol::Disabled),
        }
    }
}

impl Default for ProxyProtocol {
    fn default() -> Self {
        ProxyProtocol::Disabled
    }
}

#[derive(Debug, Clone)]
pub struct Listener(pub Option<String>, pub ProxyProtocol);

impl<'de> Deserialize<'de> for Listener {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ListenerVisitor;
        impl<'de> Visitor<'de> for ListenerVisitor {
            type Value = Listener;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("Listener")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Listener(
                    String::from(value).into(),
                    ProxyProtocol::Disabled,
                ))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Listener(None, ProxyProtocol::Disabled))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let listener = seq.next_element::<String>()?;
                let proxy = seq.next_element::<ProxyProtocol>()?.unwrap_or_default();

                Ok(Listener(listener, proxy))
            }
        }
        deserializer.deserialize_any(ListenerVisitor)
    }
}
