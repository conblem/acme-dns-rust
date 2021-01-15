use serde::de::{DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fmt::Formatter;
use std::str::FromStr;
use std::sync::Arc;
use trust_dns_server::proto::rr::{Name, RecordSet, RecordType};

#[derive(Default)]
struct PreconfiguredRecords(HashMap<Name, HashMap<&str, Arc<RecordSet>>>);

impl<'de> Deserialize<'de> for PreconfiguredRecords {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
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
                    let name = match Name::from_str(key) {
                        Err(e) => return Err(DeError::custom(e)),
                        Ok(name) => name,
                    };

                    let (record_type, record_set) =
                        map.next_value_seed(RecordDataSeed(name.clone()))?;

                    res.insert(record_type, record_set);
                }

                Ok(PreconfiguredRecords(res))
            }
        }

        deserializer.deserialize_map(PreconfiguredRecordsVisitor)
    }
}

struct RecordDataSeed(Name);

impl<'de> DeserializeSeed<'de> for RecordDataSeed {
    type Value = (RecordType, Arc<RecordSet>);

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RecordDataVisitor(Name);
        impl<'de> Visitor<'de> for RecordDataVisitor {
            type Value = (RecordType, Arc<RecordSet>);

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("RecordData")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let record_type = match seq.next_element::<&str>()? {
                    Some(record_type) => record_type,
                    None => return Err(DeError::custom("Could not find RecordType")),
                };

                let ttl = match seq.next_element::<u32>()? {
                    Some(ttl) => ttl,
                    None => return Err(DeError::custom("Could not find TTL")),
                };

                let record_set = Arc::new(RecordSet::with_ttl(self.0, record_type, ttl));

                Ok((record_type, record_set))
            }
        }

        deserializer.deserialize_seq(RecordDataVisitor(self.0))
    }
}
