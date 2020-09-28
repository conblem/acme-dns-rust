use sqlx::PgPool;
use std::future::Future;
use std::sync::Arc;
use tokio::macros::support::Pin;
use tokio::net::{ToSocketAddrs, UdpSocket};
use tokio::runtime::Runtime;
use trust_dns_client::op::LowerQuery;
use trust_dns_client::rr::{LowerName, Name};
use trust_dns_server::authority::{
    Authority, Catalog, LookupError, LookupRecords, MessageRequest, UpdateResult, ZoneType,
};
use trust_dns_server::proto::rr::dnssec::SupportedAlgorithms;
use trust_dns_server::proto::rr::rdata::TXT;
use trust_dns_server::proto::rr::record_data::RData;
use trust_dns_server::proto::rr::{Record, RecordSet, RecordType};
use trust_dns_server::ServerFuture;

use crate::cert::CertFacade;
use crate::domain::{Domain, DomainFacade};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::error::Error;
use std::str::FromStr;
use trust_dns_server::proto::rr::record_data::RData::A;

// use result instead of error
fn parse_record(
    name: &Name,
    record_type: &str,
    ttl: u32,
    mut value: Vec<String>,
) -> Option<RecordSet> {
    let record = match record_type {
        "TXT" => |val: String| Some(RData::TXT(TXT::new(vec![val]))),
        "A" => |val: String| Some(RData::A(val.parse().ok()?)),
        _ => return None,
    };

    let mut iter = value.into_iter().flat_map(record);

    let mut record_set = RecordSet::from(Record::from_rdata(name.clone(), ttl, iter.next()?));
    for record in iter {
        record_set.add_rdata(record);
    }

    Some(record_set)
}

fn parse(records: HashMap<String, Vec<Vec<String>>>) -> Option<HashMap<Name, HashMap<RecordType, RecordSet>>> {
    let result = Default::default();
    for (name, val) in records {
        let name = Name::from_str(&name).ok()?;
        for val in val {
            let mut iter = val.into_iter();
            let record_type = iter.next()?;
            let ttl = iter.next()?.parse().ok()?;
            let record_set = parse_record(&name, &record_type, ttl, iter.collect())?;
        }
    }

    Some(result)
}

pub struct DatabaseAuthority {
    lower: LowerName,
    pool: PgPool,
    name: String,
    records: Arc<HashMap<Name, HashMap<RecordType, Arc<RecordSet>>>>,
}

impl DatabaseAuthority {
    pub fn new(pool: PgPool, name: String, records: HashMap<String, Vec<Vec<String>>>) -> Self {
        let lower = LowerName::from(Name::root());

        DatabaseAuthority {
            lower,
            pool,
            name,
            records: Default::default(),
        }
    }

    fn lookup_pre(&self, name: &Name, query_type: &RecordType) -> Option<LookupRecords> {
        let record_set = Arc::clone(self.records.get(name)?.get(query_type)?);
        Some(LookupRecords::new(
            false,
            SupportedAlgorithms::new(),
            record_set,
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
        &self.lower
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
        let pool = self.pool.clone();
        let name = Name::from(query.name());
        let query_type = query.query_type();
        let pre = self.lookup_pre(&name, &query_type);

        Box::pin(async move {
            if let Some(pre) = pre {
                return Ok(pre);
            }

            if RecordType::TXT != query_type || name.len() == 0 {
                return Ok(LookupRecords::Empty);
            }

            let first = name[0].to_string();
            if first == "_acme-challenge" {
                let cert = CertFacade::first_cert(&pool).await.expect("always exists");
                let domain = DomainFacade::find_by_id(&pool, &cert.domain)
                    .await
                    .expect("always exists");

                //use match txt can be empty
                let txt = TXT::new(vec![domain.txt.unwrap()]);
                let record = Record::from_rdata(name, 100, RData::TXT(txt));
                let record = Arc::new(RecordSet::from(record));
                return Ok(LookupRecords::new(false, supported_algorithms, record));
            }

            match DomainFacade::find_by_id(&pool, &first).await {
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

pub struct DNS<'a, A> {
    server: ServerFuture<Catalog>,
    addr: A,
    runtime: &'a Runtime,
}

impl<'a, A: ToSocketAddrs> DNS<'a, A> {
    pub fn new(addr: A, runtime: &'a Runtime, authority: DatabaseAuthority) -> Self {
        let root = LowerName::from(Name::root());
        let mut catalog = Catalog::new();
        catalog.upsert(root, Box::new(authority));
        let server = ServerFuture::new(catalog);
        DNS {
            server,
            addr,
            runtime,
        }
    }

    pub async fn spawn(mut self) -> Result<(), Box<dyn Error>> {
        let udp = UdpSocket::bind(self.addr).await?;
        self.server.register_socket(udp, self.runtime);

        tokio::spawn(self.server.block_until_done()).await??;

        Ok(())
    }
}
