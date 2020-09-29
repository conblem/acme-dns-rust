use sqlx::PgPool;
use std::collections::HashMap;
use std::future::Future;
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
    records: Arc<HashMap<Name, HashMap<RecordType, Arc<RecordSet>>>>,
}

impl DatabaseAuthority {
    pub fn new(
        pool: PgPool,
        name: String,
        records: HashMap<String, Vec<Vec<String>>>,
    ) -> Box<DatabaseAuthority> {
        let lower = LowerName::from(Name::root());
        let records = Arc::new(parse(records).unwrap());

        let inner = DatabaseAuthorityInner {
            lower,
            pool,
            name,
            records,
        };

        Box::new(DatabaseAuthority(Arc::new(inner)))
    }
}

impl DatabaseAuthorityInner {
    fn lookup_pre(&self, name: &Name, query_type: &RecordType) -> Option<LookupRecords> {
        let records = &self.records;
        let record_set = Arc::clone(records.get(name)?.get(query_type)?);
        Some(LookupRecords::new(
            false,
            SupportedAlgorithms::new(),
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

        Ok(LookupRecords::new(
            false,
            SupportedAlgorithms::new(),
            record,
        ))
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
        Box::pin(async { Ok(LookupRecords::Empty) })
    }

    fn search(
        &self,
        query: &LowerQuery,
        _is_secure: bool,
        supported_algorithms: SupportedAlgorithms,
    ) -> Self::LookupFuture {
        let authority = Arc::clone(&self.0);
        let name = Name::from(query.name());
        let query_type = query.query_type();

        Box::pin(async move {
            let pre = authority.lookup_pre(&name, &query_type);
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
                    Ok(LookupRecords::new(false, supported_algorithms, record))
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
        Box::pin(async { Ok(LookupRecords::Empty) })
    }
}
