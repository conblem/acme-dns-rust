use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt::Formatter;

#[derive(Debug, Copy, Clone, PartialEq)]
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
            true => Ok(Self::Enabled),
            false => Ok(Self::Disabled),
        }
    }
}

impl Default for ProxyProtocol {
    fn default() -> Self {
        Self::Disabled
    }
}

pub type Listener = (Option<String>, ProxyProtocol);

pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Listener, D::Error>
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
            Ok((value.to_owned().into(), ProxyProtocol::Disabled))
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let listener = seq.next_element::<String>()?;
            let proxy = seq.next_element::<ProxyProtocol>()?.unwrap_or_default();

            Ok((listener, proxy))
        }
    }
    deserializer.deserialize_any(ListenerVisitor)
}
#[cfg(test)]
mod tests {
    use super::{deserialize, Listener, ProxyProtocol};
    use serde::Deserialize;
    use serde_test::{assert_de_tokens, Token};

    #[derive(Deserialize, PartialEq, Debug)]
    struct ListenerWrapper(#[serde(deserialize_with = "deserialize")] Listener);

    #[test]
    fn default() {
        assert_eq!(ProxyProtocol::Disabled, ProxyProtocol::default());
    }

    #[test]
    fn deserialize_str_test() {
        let listener = ListenerWrapper((Some("test".into()), ProxyProtocol::Disabled));

        assert_de_tokens(
            &listener,
            &[
                Token::NewtypeStruct {
                    name: "ListenerWrapper",
                },
                Token::Str("test"),
            ],
        );
    }

    #[test]
    fn deserialize_str_and_bool() {
        let listener = ListenerWrapper((Some("ip".into()), ProxyProtocol::Enabled));

        assert_de_tokens(
            &listener,
            &[
                Token::NewtypeStruct {
                    name: "ListenerWrapper",
                },
                Token::Seq { len: Some(2) },
                Token::Str("ip"),
                Token::Bool(true),
                Token::SeqEnd,
            ],
        );

        let listener = ListenerWrapper((Some("name".into()), ProxyProtocol::Disabled));

        assert_de_tokens(
            &listener,
            &[
                Token::NewtypeStruct {
                    name: "ListenerWrapper",
                },
                Token::Seq { len: Some(2) },
                Token::Str("name"),
                Token::Bool(false),
                Token::SeqEnd,
            ],
        )
    }
}
