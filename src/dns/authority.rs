use futures_util::FutureExt;
use sqlx::PgPool;
use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr::V4;
use std::pin::Pin;
use std::sync::Arc;
use trust_dns_client::op::LowerQuery;
use trust_dns_client::rr::{LowerName, Name};
use trust_dns_server::authority::{
    Authority, LookupError, LookupRecords, MessageRequest, UpdateResult, ZoneType,
};
use trust_dns_server::proto::rr::dnssec::SupportedAlgorithms;
use trust_dns_server::proto::rr::rdata::TXT;
use trust_dns_server::proto::rr::record_data::RData;
use trust_dns_server::proto::rr::{Record, RecordSet, RecordType};

use super::parse::parse;
use crate::cert::CertFacade;
use crate::domain::{Domain, DomainFacade};

pub struct DatabaseAuthority(Arc<DatabaseAuthorityInner>);

struct DatabaseAuthorityInner {
    lower: LowerName,
    pool: PgPool,
    name: String,
    records: HashMap<Name, HashMap<RecordType, Arc<RecordSet>>>,
    supported_algorithms: SupportedAlgorithms,
}

impl DatabaseAuthority {
    pub fn new(
        pool: PgPool,
        name: String,
        records: HashMap<String, Vec<Vec<String>>>,
    ) -> Box<DatabaseAuthority> {
        let lower = LowerName::from(Name::root());
        // todo: remove unwrap
        let records = parse(records).unwrap();

        let inner = DatabaseAuthorityInner {
            lower,
            pool,
            name,
            records,
            supported_algorithms: SupportedAlgorithms::new(),
        };

        Box::new(DatabaseAuthority(Arc::new(inner)))
    }
}

impl DatabaseAuthorityInner {
    async fn lookup_cname(record_set: &RecordSet) -> Option<Arc<RecordSet>> {
        let mut hosts = tokio::net::lookup_host("google.com").await.ok()?.peekable();

        // return if hosts is empty
        hosts.peek()?;

        let mut record_set = RecordSet::new(record_set.name(), RecordType::A, 0);
        for host in hosts {
            let record = match host.ip() {
                V4(ip) => RData::A(ip),
                _ => continue,
            };
            record_set.add_rdata(record);
        }

        Some(Arc::new(record_set))
    }

    async fn lookup_pre(&self, name: &Name, query_type: &RecordType) -> Option<LookupRecords> {
        let records = self.records.get(name)?;

        let record_set = match records.get(query_type) {
            Some(record_set) => Arc::clone(record_set),
            // if no A Record can be found see if maybe it is configured as a cname
            None if *query_type == RecordType::A => {
                let record_set = records.get(&RecordType::CNAME)?;
                DatabaseAuthorityInner::lookup_cname(record_set).await?
            }
            None => None?,
        };
        Some(LookupRecords::new(
            false,
            self.supported_algorithms,
            record_set,
        ))
    }

    async fn acme_challenge(&self, name: Name) -> Result<LookupRecords, LookupError> {
        let pool = &self.pool;

        let cert = CertFacade::first_cert(pool).await.expect("always exists");
        let domain = DomainFacade::find_by_id(pool, &cert.domain)
            .await
            .expect("always exists");

        //use match txt can be empty
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

        Box::pin(async move {
            let pre = authority.lookup_pre(&name, &query_type).await;
            let pool = &authority.pool;

            if let Some(pre) = pre {
                return Ok(pre);
            }

            if RecordType::TXT != query_type || name.len() == 0 {
                return Ok(LookupRecords::Empty);
            }

            let first = name[0].to_string();
            if first == "_acme-challenge" {
                return authority.acme_challenge(name).await;
            }

            match DomainFacade::find_by_id(pool, &first).await {
                Some(Domain { txt: Some(txt), .. }) => {
                    let txt = TXT::new(vec![txt]);
                    let record = Record::from_rdata(name, 100, RData::TXT(txt));
                    let record = Arc::new(RecordSet::from(record));
                    Ok(LookupRecords::new(
                        false,
                        authority.supported_algorithms,
                        record,
                    ))
                }
                _ => Ok(LookupRecords::Empty),
            }
        })
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
