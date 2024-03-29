use anyhow::{anyhow, Result};
use futures_util::TryFutureExt;
use std::io::{Error as IoError, ErrorKind};
use std::net::IpAddr::V4;
use std::str;
use std::str::FromStr;
use std::sync::Arc;
use tracing::field::display;
use tracing::{debug, error, info, Instrument, Span};
use trust_dns_server::authority::{
    AuthorityObject, BoxedLookupFuture, LookupObject, LookupRecords, MessageRequest, UpdateResult,
    ZoneType,
};
use trust_dns_server::client::op::LowerQuery;
use trust_dns_server::client::rr::LowerName;
use trust_dns_server::proto::rr::dnssec::SupportedAlgorithms;
use trust_dns_server::proto::rr::rdata::{SOA, TXT};
use trust_dns_server::proto::rr::record_data::RData;
use trust_dns_server::proto::rr::{Name, Record, RecordSet, RecordType};

use crate::config::PreconfiguredRecords;
use crate::facade::{CertFacade, Domain, DomainFacade};
use crate::util::error;

pub struct DatabaseAuthority<F>(Arc<DatabaseAuthorityInner<F>>);

struct DatabaseAuthorityInner<F> {
    lower: LowerName,
    facade: F,
    records: PreconfiguredRecords,
    supported_algorithms: SupportedAlgorithms,
}

impl<F> DatabaseAuthority<F> {
    pub fn new(facade: F, name: &str, records: PreconfiguredRecords) -> Box<Self> {
        // todo: remove unwrap
        let lower = LowerName::from(Name::from_str(name).unwrap());
        // todo: remove unwrap

        let inner = DatabaseAuthorityInner {
            lower,
            facade,
            records,
            supported_algorithms: SupportedAlgorithms::new(),
        };

        Box::new(DatabaseAuthority(Arc::new(inner)))
    }
}

#[tracing::instrument(skip(record_set))]
async fn lookup_cname(record_set: &RecordSet) -> Result<Option<Arc<RecordSet>>> {
    let name = record_set.name();
    let records = record_set
        .records_without_rrsigs()
        .next()
        .map(Record::rdata);

    let cname = match records {
        Some(RData::CNAME(cname)) => cname,
        _ => return Ok(None),
    };

    // hack tokio expects a socket addr
    let addr = format!("{}:80", cname);
    debug!("resolving following cname ip {}", addr);
    let hosts = tokio::net::lookup_host(addr).await?;

    let mut record_set = RecordSet::new(name, RecordType::A, 0);
    for host in hosts {
        let record = match host.ip() {
            V4(ip) => RData::A(ip),
            _ => continue,
        };
        record_set.add_rdata(record);
    }

    if record_set.is_empty() {
        debug!("dns lookup returned no ipv4 records");
        return Ok(None);
    }

    Ok(Some(Arc::new(record_set)))
}

impl<F: DomainFacade + CertFacade> DatabaseAuthorityInner<F> {
    #[tracing::instrument(err, skip(self, name, query_type))]
    async fn lookup_pre(
        &self,
        name: &Name,
        query_type: &RecordType,
    ) -> Result<Option<LookupRecords>> {
        debug!("Starting Prelookup");
        let records = match self.records.get(name) {
            Some(records) => records,
            None => {
                debug!("Empty Prelookup");
                return Ok(None);
            }
        };

        let record_set = match (records.get(query_type), records.get(&RecordType::CNAME)) {
            (Some(record_set), _) => Some(Arc::clone(record_set)),
            // if no A Record can be found, see if maybe it is configured as a cname
            (None, Some(record_set)) if *query_type == RecordType::A => {
                lookup_cname(record_set).await?
            }
            (None, _) => {
                debug!("Empty Prelookup");
                return Ok(None);
            }
        };

        match record_set {
            Some(record_set) => {
                debug!("pre lookup resolved: {:?}", record_set);
                Ok(Some(LookupRecords::new(
                    false,
                    self.supported_algorithms,
                    record_set,
                )))
            }
            None => {
                debug!("Empty Prelookup");
                Ok(None)
            }
        }
    }

    #[tracing::instrument(skip(self, name))]
    async fn acme_challenge(&self, name: Name) -> Result<LookupRecords> {
        let cert = match self.facade.first_cert().await? {
            Some(cert) => cert,
            None => return Err(anyhow!("First cert not found")),
        };
        let domain = match self.facade.find_domain_by_id(&cert.domain).await {
            Ok(Some(domain)) => domain,
            Ok(None) => return Err(anyhow!("Domain not found {}", cert.domain)),
            Err(e) => return Err(e.into()),
        };

        // todo: use match txt can be empty
        let txt = TXT::new(vec![domain.txt.unwrap()]);
        let record = Record::from_rdata(name, 100, RData::TXT(txt));
        let record = Arc::new(RecordSet::from(record));

        Ok(LookupRecords::new(false, self.supported_algorithms, record))
    }
}

#[allow(dead_code)]
impl<F: DomainFacade + CertFacade + Send + Sync + 'static> AuthorityObject
    for DatabaseAuthority<F>
{
    fn box_clone(&self) -> Box<(dyn AuthorityObject + 'static)> {
        Box::new(DatabaseAuthority(Arc::clone(&self.0)))
    }

    fn zone_type(&self) -> ZoneType {
        ZoneType::Primary
    }

    fn is_axfr_allowed(&self) -> bool {
        false
    }

    fn update(&self, _update: &MessageRequest) -> UpdateResult<bool> {
        Ok(false)
    }

    fn origin(&self) -> LowerName {
        self.0.lower.clone()
    }

    fn lookup(
        &self,
        _name: &LowerName,
        _rtype: RecordType,
        _is_secure: bool,
        _supported_algorithms: SupportedAlgorithms,
    ) -> BoxedLookupFuture {
        BoxedLookupFuture::empty()
    }

    fn search(
        &self,
        query: &LowerQuery,
        _is_secure: bool,
        _supported_algorithms: SupportedAlgorithms,
    ) -> BoxedLookupFuture {
        let authority = Arc::clone(&self.0);
        let name = Name::from(query.name());
        let query_type = query.query_type();
        let span = Span::current();
        span.record("name", &display(&name));
        span.record("query_type", &display(&query_type));

        // not sure if this handling makes sense
        if query_type == RecordType::SOA {
            return span.in_scope(|| self.soa());
        }

        BoxedLookupFuture::from(
            async move {
                info!("Starting lookup");
                let first = match name.iter().next() {
                    Some(first) => first,
                    None => return Ok(LookupRecords::Empty),
                };

                if name.is_empty() {
                    return Ok(LookupRecords::Empty);
                }

                // no error handling needed we just try the other lookups
                if let Ok(Some(pre)) = authority.lookup_pre(&name, &query_type).await {
                    return Ok(pre);
                }

                if first == b"_acme-challenge" {
                    return authority.acme_challenge(name).await.map_err(error);
                }

                let first = match str::from_utf8(first) {
                    Ok(first) => first,
                    Err(e) => return Err(error(e)),
                };

                let txt = match authority.facade.find_domain_by_id(first).await {
                    Ok(Some(Domain { txt: Some(txt), .. })) => txt,
                    Ok(Some(Domain { txt: None, .. })) => return Ok(LookupRecords::Empty),
                    Ok(None) => return Err(error(IoError::from(ErrorKind::NotFound))),
                    Err(e) => return Err(error(e)),
                };
                let txt = TXT::new(vec![txt]);
                let record = Record::from_rdata(name, 100, RData::TXT(txt));
                let record_set = Arc::new(RecordSet::from(record));

                Ok(LookupRecords::new(
                    false,
                    authority.supported_algorithms,
                    record_set,
                ))
            }
            .map_ok(|res| Box::new(res) as Box<dyn LookupObject>)
            .inspect_err(|err| error!("{}", err))
            .instrument(span),
        )
    }

    fn get_nsec_records(
        &self,
        _name: &LowerName,
        _is_secure: bool,
        _supported_algorithms: SupportedAlgorithms,
    ) -> BoxedLookupFuture {
        BoxedLookupFuture::empty()
    }

    // fix handling of this as this always take self.origin
    // also admin is always same serial numbers need to match
    fn soa(&self) -> BoxedLookupFuture {
        let origin: Name = self.origin().into();
        let supported_algorithms = self.0.supported_algorithms;
        BoxedLookupFuture::from(
            async move {
                let soa = SOA::new(
                    origin.clone(),
                    origin.clone(),
                    1,
                    28800,
                    7200,
                    604800,
                    86400,
                );
                let record = Record::from_rdata(origin, 100, RData::SOA(soa));
                let record_set = RecordSet::from(record);
                let records =
                    LookupRecords::new(false, supported_algorithms, Arc::from(record_set));
                let records = Box::new(records) as Box<dyn LookupObject>;
                Ok(records)
            }
            .in_current_span(),
        )
    }

    fn soa_secure(
        &self,
        _is_secure: bool,
        _supported_algorithms: SupportedAlgorithms,
    ) -> BoxedLookupFuture {
        self.soa()
    }
}

#[cfg(test)]
mod tests {
    use crate::dns::authority::lookup_cname;
    use std::net::Ipv4Addr;
    use std::str::FromStr;
    use trust_dns_server::proto::rr::{Name, RData, Record, RecordType};

    #[tokio::test]
    async fn lookup_cname_works() {
        let name = Name::from_str("test.domain.com").expect("Could not parse name");
        let lookup = Name::from_str("example.com").expect("Could not parse name");
        let record_set = Record::from_rdata(name, 100, RData::CNAME(lookup)).into();

        let actual = match lookup_cname(&record_set).await {
            Ok(Some(actual)) => actual,
            _ => panic!("Could not resolve cname"),
        };

        let record = actual
            .records_without_rrsigs()
            .next()
            .expect("no records in recordset");
        assert_eq!(RecordType::A, record.record_type());

        let ip = match record.rdata() {
            RData::A(ip) => ip,
            _ => panic!("Resolved record is not of a type"),
        };

        let expected: Ipv4Addr = "93.184.216.34".parse().expect("Could not parse ip");
        assert_eq!(&expected, ip);
    }
}
