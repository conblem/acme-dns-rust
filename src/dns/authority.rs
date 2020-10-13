use futures_util::FutureExt;
use sqlx::PgPool;
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::IpAddr::V4;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use trust_dns_client::op::LowerQuery;
use trust_dns_client::rr::{LowerName, Name};
use trust_dns_server::authority::{
    Authority, LookupError, LookupRecords, MessageRequest, UpdateResult, ZoneType,
};
use trust_dns_server::proto::op::ResponseCode;
use trust_dns_server::proto::rr::dnssec::SupportedAlgorithms;
use trust_dns_server::proto::rr::rdata::TXT;
use trust_dns_server::proto::rr::record_data::RData;
use trust_dns_server::proto::rr::{Record, RecordSet, RecordType};
use anyhow::anyhow;

use super::parse::parse;
use crate::cert::CertFacade;
use crate::domain::{Domain, DomainFacade};
use crate::util::error;

pub struct DatabaseAuthority(Arc<DatabaseAuthorityInner>);

struct DatabaseAuthorityInner {
    lower: LowerName,
    pool: PgPool,
    records: HashMap<Name, HashMap<RecordType, Arc<RecordSet>>>,
    supported_algorithms: SupportedAlgorithms,
}

impl DatabaseAuthority {
    pub fn new(
        pool: PgPool,
        name: &str,
        records: HashMap<String, Vec<Vec<String>>>,
    ) -> Box<DatabaseAuthority> {
        // todo: remove unwrap
        let lower = LowerName::from(Name::from_str(name).unwrap());
        // todo: remove unwrap
        let records = parse(records).unwrap();

        let inner = DatabaseAuthorityInner {
            lower,
            pool,
            records,
            supported_algorithms: SupportedAlgorithms::new(),
        };

        Box::new(DatabaseAuthority(Arc::new(inner)))
    }
}

async fn lookup_cname(record_set: &RecordSet) -> Result<Arc<RecordSet>, LookupError> {
    let name = record_set.name();
    let records = record_set
        .records_without_rrsigs()
        .next()
        .map(Record::rdata);

    let cname = match records {
        Some(RData::CNAME(cname)) => cname,
        _ => return Err(LookupError::ResponseCode(ResponseCode::ServFail)),
    };

    // hack tokio expects a socket addr
    let addr = format!("{}:80", cname);
    log::debug!("resolving following cname ip {}", addr);
    let hosts = tokio::net::lookup_host(addr)
        .await
        .map_err(|_| LookupError::ResponseCode(ResponseCode::ServFail))?;

    let mut record_set = RecordSet::new(name, RecordType::A, 0);
    for host in hosts {
        let record = match host.ip() {
            V4(ip) => RData::A(ip),
            _ => continue,
        };
        record_set.add_rdata(record);
    }

    if record_set.is_empty() {
        log::debug!("dns lookup returned no ipv4 records");
        return Err(LookupError::ResponseCode(ResponseCode::ServFail));
    }

    Ok(Arc::new(record_set))
}

impl DatabaseAuthorityInner {
    async fn lookup_pre(
        &self,
        name: &Name,
        query_type: &RecordType,
    ) -> Result<Option<LookupRecords>, LookupError> {
        log::debug!("starting prelookup for {}, {}", name, query_type);
        let records = match self.records.get(name) {
            Some(records) => records,
            None => return Ok(None),
        };

        let record_set = match (records.get(query_type), records.get(&RecordType::CNAME)) {
            (Some(record_set), _) => Arc::clone(record_set),
            // if no A Record can be found, see if maybe it is configured as a cname
            (None, Some(record_set)) if *query_type == RecordType::A => {
                lookup_cname(record_set).await?
            }
            (None, _) => return Ok(None),
        };
        log::debug!("pre lookup resolved: {:?}", record_set);
        Ok(Some(LookupRecords::new(
            false,
            self.supported_algorithms,
            record_set,
        )))
    }

    async fn acme_challenge(&self, name: Name) -> Result<LookupRecords, LookupError> {
        let pool = &self.pool;

        let cert = match CertFacade::first_cert(pool).await {
            Ok(Some(cert)) => cert,
            Ok(None) => return Err(error(anyhow!("First cert not found"))),
            Err(e) => return Err(error(e)),
        };
        let domain = match DomainFacade::find_by_id(pool, &cert.domain).await {
            Ok(Some(domain)) => domain,
            Ok(None) => return Err(error(anyhow!("Domain not found {}", cert.domain))),
            Err(e) => return Err(error(e)),
        };

        // todo: use match txt can be empty
        let txt = TXT::new(vec![domain.txt.unwrap()]);
        let record = Record::from_rdata(name, 100, RData::TXT(txt));
        let record = Arc::new(RecordSet::from(record));

        Ok(LookupRecords::new(false, self.supported_algorithms, record))
    }
}

#[allow(dead_code)]
impl Authority for DatabaseAuthority {
    type Lookup = LookupRecords;
    type LookupFuture = Pin<Box<dyn Future<Output = Result<Self::Lookup, LookupError>> + Send>>;

    fn zone_type(&self) -> ZoneType {
        ZoneType::Master
    }

    fn is_axfr_allowed(&self) -> bool {
        false
    }

    fn update(&mut self, _update: &MessageRequest) -> UpdateResult<bool> {
        Ok(false)
    }

    fn origin(&self) -> &LowerName {
        &self.0.lower
    }

    fn lookup(
        &self,
        _name: &LowerName,
        _rtype: RecordType,
        _is_secure: bool,
        _supported_algorithms: SupportedAlgorithms,
    ) -> Self::LookupFuture {
        futures_util::future::ok(LookupRecords::Empty).boxed()
    }

    fn search(
        &self,
        query: &LowerQuery,
        _is_secure: bool,
        _supported_algorithms: SupportedAlgorithms,
    ) -> Self::LookupFuture {
        let authority = Arc::clone(&self.0);
        let name = Name::from(query.name());
        let query_type = query.query_type();

        async move {
            if name.is_empty() {
                return Ok(LookupRecords::Empty);
            }

            // todo: don't abort on error still continue with normal lookup
            if let Some(pre) = authority.lookup_pre(&name, &query_type).await? {
                return Ok(pre);
            }

            let first = name[0].to_string();
            if first == "_acme-challenge" {
                return authority.acme_challenge(name).await;
            }

            let pool = &authority.pool;

            let txt = match DomainFacade::find_by_id(pool, &first).await {
                Ok(Some(Domain { txt: Some(txt), .. })) => txt,
                Ok(None) => return Err(LookupError::Io(io::Error::from(io::ErrorKind::NotFound))),
                Err(e) => return Err(LookupError::Io(io::Error::new(io::ErrorKind::Other, e))),
                _ => return Ok(LookupRecords::Empty),
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
        .boxed()
    }

    fn get_nsec_records(
        &self,
        _name: &LowerName,
        _is_secure: bool,
        _supported_algorithms: SupportedAlgorithms,
    ) -> Self::LookupFuture {
        futures_util::future::ok(LookupRecords::Empty).boxed()
    }
}
