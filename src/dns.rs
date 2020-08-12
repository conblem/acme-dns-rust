use trust_dns_server::authority::{Catalog, ZoneType, Authority, MessageRequest, LookupError, UpdateResult, LookupObject, LookupRecords};
use std::sync::Arc;
use trust_dns_server::ServerFuture;
use trust_dns_server::server::{RequestHandler, ResponseHandler, Request};
use std::ops::Deref;
use std::str::FromStr;
use trust_dns_client::rr::{Name, LowerName};
use trust_dns_server::store::in_memory::InMemoryAuthority;
use tokio::net::UdpSocket;
use tokio::runtime::Runtime;
use trust_dns_server::proto::rr::{Record, RecordType, RecordSet, IntoName};
use trust_dns_server::proto::rr::record_data::RData;
use trust_dns_server::proto::rr::rdata::TXT;
use trust_dns_server::proto::rr::dnssec::SupportedAlgorithms;
use trust_dns_client::op::LowerQuery;
use tokio::macros::support::Pin;
use std::future::Future;
use std::borrow::Borrow;
use sqlx::{Database, Postgres, Pool};

use crate::domain::{DomainFacade, Domain};
use std::marker::PhantomData;
use trust_dns_server::proto::rr::domain::Label;
use crate::cert::CertFacade;

struct DatabaseAuthority<DB: Database> {
    lower: LowerName,
    pool: Pool<DB>
}

impl <DB: Database> DatabaseAuthority<DB> {
    fn new(pool: Pool<DB>) -> Self {
        let lower = LowerName::from(Name::root());

        DatabaseAuthority {
            lower,
            pool
        }
    }
}

#[allow(dead_code)]
impl Authority for DatabaseAuthority<Postgres> {
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

    fn lookup(&self, _name: &LowerName, _rtype: RecordType, _is_secure: bool, _supported_algorithms: SupportedAlgorithms) -> Self::LookupFuture {
        Box::pin(async {
            Ok(LookupRecords::Empty)
        })
    }

    fn search(&self, query: &LowerQuery, _is_secure: bool, supported_algorithms: SupportedAlgorithms) -> Self::LookupFuture {
        let name = Name::from(query.name());

        if RecordType::TXT != query.query_type() || name.len() == 0 {
            return Box::pin(async {
                Ok(LookupRecords::Empty)
            })
        }

        let first= name[0].to_string();
        let mut pool = self.pool.clone();

        if first == "_acme-challenge" {
            return Box::pin(async move {
                let cert = CertFacade::first_cert(&pool).await.expect("always exists");
                let domain = DomainFacade::find_by_id(&pool, &cert.domain).await.expect("always exists");

                //use match
                let txt = TXT::new(vec![domain.txt.unwrap()]);
                let record = Record::from_rdata(
                    name,
                    100,
                    RData::TXT(txt)
                );
                let record = Arc::new(RecordSet::from(record));
                Ok(LookupRecords::new(false, supported_algorithms, record))
            });
        }

        Box::pin(async move {
            match DomainFacade::find_by_id(&pool, &first).await {
                Some(Domain { txt: Some(txt), .. }) => {
                    let txt = TXT::new(vec![txt]);
                    let record = Record::from_rdata(
                        name,
                        100,
                        RData::TXT(txt)
                    );
                    let record = Arc::new(RecordSet::from(record));
                    Ok(LookupRecords::new(false, supported_algorithms, record))
                },
                _ => {
                    Ok(LookupRecords::Empty)
                }
            }
        })
    }

    fn get_nsec_records(&self, _name: &LowerName, _is_secure: bool, _supported_algorithms: SupportedAlgorithms) -> Self::LookupFuture{
        Box::pin(async {
            Ok(LookupRecords::Empty)
        })
    }
}

pub struct DNS<DB: Database> {
    server: ServerFuture<Catalog>,
    _phantom: PhantomData<DB>
}

impl DNS<Postgres> {
    pub fn new(pool: Pool<Postgres>) -> Self {
        let root = LowerName::from(Name::root());
        let mut catalog = Catalog::new();
        catalog.upsert(root, Box::new(DatabaseAuthority::new(pool)));
        let server = ServerFuture::new(catalog);

        DNS {
            server,
            _phantom: PhantomData
        }
    }
}

impl <DB: Database> DNS<DB> {
    pub fn run(&mut self, udp: UdpSocket, runtime: &Runtime) {
        self.server.register_socket(udp, runtime);
    }

    pub async fn block_until_done(self) {
        self.server.block_until_done().await.unwrap();
    }
}

