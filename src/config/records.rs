use serde::de::{DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;
use std::collections::HashMap;
use std::fmt::Formatter;
use std::str::FromStr;
use std::sync::Arc;
use trust_dns_server::proto::rr::rdata::TXT;
use trust_dns_server::proto::rr::{Name, RData, RecordSet, RecordType};

pub type PreconfiguredRecords = HashMap<Name, HashMap<RecordType, Arc<RecordSet>>>;

pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<PreconfiguredRecords, D::Error>
where
    D: Deserializer<'de>,
{
    struct PreconfiguredRecordsVisitor;
    impl<'de> Visitor<'de> for PreconfiguredRecordsVisitor {
        type Value = PreconfiguredRecords;

        fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
            formatter.write_str("PreconfiguredRecords")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut res = HashMap::new();
            while let Some(key) = map.next_key::<&str>()? {
                let mut name = Name::from_str(key).map_err(DeError::custom)?;
                name.set_fqdn(true);
                let (record_type, record_set) = map.next_value_seed(RecordDataSeed(name))?;

                res.insert(record_type, record_set);
            }

            Ok(res)
        }
    }

    deserializer.deserialize_map(PreconfiguredRecordsVisitor)
}

struct RecordDataSeed(Name);

impl<'de> DeserializeSeed<'de> for RecordDataSeed {
    type Value = (Name, HashMap<RecordType, Arc<RecordSet>>);

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RecordDataVisitor(Name);
        impl<'de> Visitor<'de> for RecordDataVisitor {
            type Value = (Name, HashMap<RecordType, Arc<RecordSet>>);

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("RecordData")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let name = self.0;
                let mut res = HashMap::with_capacity(map.size_hint().unwrap_or_default());
                while let Some(record_type) = map.next_key::<&str>()? {
                    let record_type = match record_type {
                        "TXT" => RecordType::TXT,
                        "A" => RecordType::A,
                        "CNAME" => RecordType::CNAME,
                        _ => return Err(DeError::custom("Could not find RecordType")),
                    };

                    let record_set = map.next_value_seed(RecordSeed(name.clone(), record_type))?;

                    res.insert(record_type, record_set);
                }

                Ok((name, res))
            }
        }

        deserializer.deserialize_map(RecordDataVisitor(self.0))
    }
}

struct RecordSeed(Name, RecordType);

impl<'de> DeserializeSeed<'de> for RecordSeed {
    type Value = Arc<RecordSet>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RecordVisitor(Name, RecordType);
        impl<'de> Visitor<'de> for RecordVisitor {
            type Value = Arc<RecordSet>;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("Record")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let ttl = match seq.next_element::<u32>()? {
                    Some(ttl) => ttl,
                    None => return Err(DeError::custom("Could not find TTL")),
                };

                let mut record_set = RecordSet::with_ttl(self.0, self.1, ttl);
                while let Some(data) = seq.next_element::<&str>()? {
                    let rdata = match self.1 {
                        RecordType::A => RData::A(data.parse().map_err(DeError::custom)?),
                        RecordType::TXT => RData::TXT(TXT::new(vec![data.into()])),
                        RecordType::CNAME => RData::CNAME(data.parse().map_err(DeError::custom)?),
                        _ => return Err(DeError::custom("Invalid key")),
                    };
                    match record_set.add_rdata(rdata) {
                        true => continue,
                        false => {
                            return Err(DeError::custom(format!(
                                "Could not insert data {} {}",
                                self.1, data
                            )))
                        }
                    }
                }

                Ok(Arc::new(record_set))
            }
        }

        deserializer.deserialize_seq(RecordVisitor(self.0, self.1))
    }
}

#[cfg(test)]
mod tests {
    use super::{deserialize, PreconfiguredRecords};
    use serde::Deserialize;
    use serde_test::{assert_de_tokens, Token};

    #[derive(Deserialize, PartialEq, Debug)]
    struct PreconfiguredRecordsWrapper(
        #[serde(deserialize_with = "deserialize")] PreconfiguredRecords,
    );

    #[test]
    fn deserialize_test() {
        let records = Default::default();
        let records = PreconfiguredRecordsWrapper(records);

        assert_de_tokens(
            &records,
            &[
                Token::NewtypeStruct {
                    name: "PreconfiguredRecordsWrapper",
                },
                Token::Map { len: Some(1) },
                Token::BorrowedStr("acme.example.com"),
                Token::Map { len: Some(1) },
                Token::BorrowedStr("A"),
                Token::Seq { len: Some(2) },
                Token::U32(100),
                Token::BorrowedStr("1.1.1.1"),
                Token::SeqEnd,
                Token::MapEnd,
                Token::MapEnd,
            ],
        )
    }
}
