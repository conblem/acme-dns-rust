use serde::de::{DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fmt::Formatter;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use trust_dns_server::proto::rr::{Name, RecordSet, RecordType, RData};

#[derive(Debug, Default)]
pub struct PreconfiguredRecords(HashMap<Name, HashMap<RecordType, Arc<RecordSet>>>);

impl Deref for PreconfiguredRecords {
    type Target = HashMap<Name, HashMap<RecordType, Arc<RecordSet>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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
                let mut res = HashMap::new();
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

                // todo: finish this code
                let mut record_set = RecordSet::with_ttl(self.0, self.1, ttl);
                while let Some(data) = seq.next_element::<&str>()? {
                    match self.1 {
                        _ => return Err(DeError::custom("Invalid key"))
                    }
                }

                Ok(Arc::new(record_set))
            }
        }

        deserializer.deserialize_seq(RecordVisitor(self.0, self.1))
    }
}
