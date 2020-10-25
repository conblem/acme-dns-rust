use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use trust_dns_server::proto::rr::rdata::TXT;
use trust_dns_server::proto::rr::{Name, RData, Record, RecordSet, RecordType};

// todo: use result instead of option
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
        _ => return None,
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
    use std::net::Ipv4Addr;
    use std::str::FromStr;
    use trust_dns_server::proto::rr::{Name, RData, Record, RecordType};

    #[test]
    fn parse_txt_record_works() {
        let name = Name::from_str("google.com").expect("Unable to parse name");
        let data = vec!["Hallo".to_string(), "Welt".to_string()].into_iter();
        let record = parse_record(&name, "TXT", 100, data).expect("Could not parse record");

        assert!(!record.is_empty());
        assert_eq!(RecordType::TXT, record.record_type());

        let mut records = record.records_without_rrsigs();
        let record = records.next().expect("There is no record");
        compare_txt(record, 100, "Hallo");

        let record = records.next().expect("There is no record");
        compare_txt(record, 100, "Welt");
    }

    fn compare_txt(record: &Record, ttl: u32, data: &str) {
        assert_eq!(RecordType::TXT, record.record_type());
        assert_eq!(ttl, record.ttl());

        let txt = match record.rdata() {
            RData::TXT(txt) => txt,
            _ => panic!("RData is not TXT"),
        };

        assert_eq!(data.as_bytes(), &*txt.txt_data()[0])
    }

    // todo: fix this test on ci
    //#[test]
    fn parse_a_record_works() {
        let name = Name::from_str("google.com").expect("Unable to parse name");
        let data = vec!["1.1.1.1".to_string(), "2.2.2.2".to_string()].into_iter();
        let record = parse_record(&name, "A", 100, data).expect("Could not parse record");

        assert!(!record.is_empty());
        assert_eq!(RecordType::A, record.record_type());

        let mut records = record.records_without_rrsigs();
        let record = records.next().expect("There is no record");
        compare_a(record, 100, "1.1.1.1".parse().expect("Is not a IP"));

        let record = records.next().expect("There is no record");
        compare_a(record, 100, "2.2.2.2".parse().expect("Is not a IP"));
    }

    fn compare_a(record: &Record, ttl: u32, data: Ipv4Addr) {
        assert_eq!(RecordType::A, record.record_type());
        assert_eq!(ttl, record.ttl());

        let ip = match record.rdata() {
            RData::A(ip) => ip,
            _ => panic!("RData is not A"),
        };

        assert_eq!(&data, ip)
    }

    // there is only one cname record supported
    #[test]
    fn parse_cname_record_works() {
        let name = Name::from_str("google.com").expect("Unable to parse name");
        let data = vec!["test.com".to_string()].into_iter();
        let record = parse_record(&name, "CNAME", 100, data).expect("Could not parse record");

        assert!(!record.is_empty());
        assert_eq!(RecordType::CNAME, record.record_type());

        let record = record
            .records_without_rrsigs()
            .next()
            .expect("There is no record");

        assert_eq!(RecordType::CNAME, record.record_type());
        assert_eq!(100, record.ttl());

        let actual = match record.rdata() {
            RData::CNAME(actual) => actual,
            _ => panic!("RData is not CNAME"),
        };

        let mut expected = Name::from_str("test.com").expect("Is not a valid name");
        expected.set_fqdn(true);
        assert_eq!(&expected, actual)
    }

    #[test]
    fn parse_invalid_record_does_not_work() {
        let name = Name::from_str("google.com").expect("Unable to parse name");
        let data = vec!["test.com".to_string()].into_iter();
        let records = parse_record(&name, "ALIAS", 100, data);
        assert_eq!(None, records);
    }
}
