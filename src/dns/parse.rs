use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use trust_dns_server::proto::rr::rdata::TXT;
use trust_dns_server::proto::rr::{Name, RData, Record, RecordSet, RecordType};

// use result instead of error
fn parse_record(
    name: &Name,
    record_type: &str,
    ttl: u32,
    value: impl Iterator<Item = String>,
) -> Option<RecordSet> {
    let record: fn(String) -> Option<RData> = match record_type {
        "TXT" => |val| Some(RData::TXT(TXT::new(vec![val]))),
        "A" => |val| Some(RData::A(val.parse().ok()?)),
        "CNAME" => |val| Some(RData::CNAME(Name::from_str(&val).ok()?)),
        _ => None?,
    };

    let mut iter = value.flat_map(record);
    // returns here if iter is empty
    let record = Record::from_rdata(name.clone(), ttl, iter.next()?);
    let mut record_set = RecordSet::from(record);

    for record in iter {
        record_set.add_rdata(record);
    }

    Some(record_set)
}

// todo: improve error handling and naming
pub(super) fn parse(
    records: HashMap<String, Vec<Vec<String>>>,
) -> Option<HashMap<Name, HashMap<RecordType, Arc<RecordSet>>>> {
    let mut result: HashMap<Name, HashMap<RecordType, Arc<RecordSet>>> = Default::default();

    for (name, val) in records {
        let mut name = match Name::from_str(&name) {
            Ok(name) => name,
            Err(_) => {
                log::error!("Could not parse name {}, skipping", name);
                continue;
            }
        };
        name.set_fqdn(true);
        for val in val {
            let mut iter = val.into_iter();
            let record_type = iter.next()?;
            let ttl = iter.next()?.parse().ok()?;
            let record_set = parse_record(&name, &record_type, ttl, iter)?;
            let record_type = record_set.record_type();
            result
                .entry(name.clone())
                .or_default()
                .insert(record_type, Arc::new(record_set));
        }
    }

    log::debug!("records parsed {:?}", result);
    Some(result)
}

#[cfg(test)]
mod tests {
    use crate::dns::parse::parse_record;
    use std::str::FromStr;
    use trust_dns_server::proto::rr::{Name, RData, RecordType};

    #[test]
    fn parse_txt_record_works() {
        let name = Name::from_str("google.com").expect("Unable to parse name");
        let data = vec!["Hallo".to_string()].into_iter();
        let record = parse_record(&name, "TXT", 100, data).expect("Could not parse record");

        assert!(!record.is_empty());
        assert_eq!(RecordType::TXT, record.record_type());

        let record = record
            .records_without_rrsigs()
            .next()
            .expect("There is no record");
        assert_eq!(RecordType::TXT, record.record_type());
        assert_eq!(100, record.ttl());

        let txt = match record.rdata() {
            RData::TXT(txt) => txt,
            _ => panic!("RData is not TXT"),
        };

        assert_eq!(b"Hallo", &*txt.txt_data()[0])
    }
}
