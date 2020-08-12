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
use sqlx::{Database, Postgres};

use crate::domain::{DomainFacade, Domain};
use std::marker::PhantomData;

struct DatabaseAuthority<DB: Database> {
    lower: LowerName,
    domain_facade: DomainFacade<DB>
}

impl <DB: Database> DatabaseAuthority<DB> {
    fn new(domain_facade: DomainFacade<DB>) -> Self {
        let lower = LowerName::from(Name::root());

        DatabaseAuthority {
            lower,
            domain_facade
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

    fn update(&mut self, update: &MessageRequest) -> UpdateResult<bool> {
        Ok(false)
    }

    fn origin(&self) -> &LowerName {
        &self.lower
    }

    fn lookup(&self, name: &LowerName, rtype: RecordType, is_secure: bool, supported_algorithms: SupportedAlgorithms) -> Self::LookupFuture {
        Box::pin(async {
            println!("lookup");
            Ok(LookupRecords::Empty)
        })
    }

    fn search(&self, query: &LowerQuery, is_secure: bool, supported_algorithms: SupportedAlgorithms) -> Self::LookupFuture {
        if RecordType::TXT != query.query_type() {
            return Box::pin(async {
                Ok(LookupRecords::Empty)
            })
        }

        let name = Borrow::<Name>::borrow(query.name()).clone();
        let domain_facade = self.domain_facade.clone();

        Box::pin(async move {
            println!("{:?}", &name.to_string());
            match domain_facade.find_by_id(&name.to_string()).await {
                Some(Domain { txt: Some(txt), .. }) => {
                    let txt = TXT::new(vec![txt]);
                    let record = Record::from_rdata(
                        name,
                        //Name::from_str("example.com").unwrap(),
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

    fn get_nsec_records(&self, name: &LowerName, is_secure: bool, supported_algorithms: SupportedAlgorithms) -> Self::LookupFuture{
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
    pub fn new(domain_facade: DomainFacade<Postgres>) -> Self {
        let root = LowerName::from(Name::root());
        let mut catalog = Catalog::new();
        catalog.upsert(root, Box::new(DatabaseAuthority::new(domain_facade)));
        let server = ServerFuture::new(catalog);

        DNS {
            server,
            _phantom: PhantomData
        }
    }

    pub fn run(&mut self, udp: UdpSocket, runtime: &Runtime) {
        self.server.register_socket(udp, runtime);
    }

    pub async fn block_until_done(self) {
        self.server.block_until_done().await.unwrap();
    }
}

