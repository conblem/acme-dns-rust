use sqlx::AnyPool;
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
use std::error::Error;

struct DatabaseAuthority {
    lower: LowerName,
    pool: AnyPool,
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
        let name = Name::from(query.name());

        if RecordType::TXT != query.query_type() || name.len() == 0 {
            return Box::pin(async { Ok(LookupRecords::Empty) });
        }

        let first = name[0].to_string();
        let pool = self.pool.clone();

        if first == "_acme-challenge" {
            return Box::pin(async move {
                let cert = CertFacade::first_cert(&pool).await.expect("always exists");
                let domain = DomainFacade::find_by_id(&pool, &cert.domain)
                    .await
                    .expect("always exists");

                //use match
                let txt = TXT::new(vec![domain.txt.unwrap()]);
                let record = Record::from_rdata(name, 100, RData::TXT(txt));
                let record = Arc::new(RecordSet::from(record));
                Ok(LookupRecords::new(false, supported_algorithms, record))
            });
        }

        Box::pin(async move {
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

impl DatabaseAuthority {
    fn new(pool: AnyPool) -> Self {
        let lower = LowerName::from(Name::root());

        DatabaseAuthority { lower, pool }
    }
}

pub struct DNS {
    server: ServerFuture<Catalog>,
}

impl DNS {
    pub async fn builder<A: ToSocketAddrs>(addr: A) -> tokio::io::Result<DNSBuilder> {
        let udp = UdpSocket::bind(addr).await?;

        Ok(DNSBuilder(udp))
    }

    pub async fn spawn(self) -> Result<(), Box<dyn Error>> {
        tokio::spawn(self.server.block_until_done()).await??;

        Ok(())
    }
}

pub struct DNSBuilder(UdpSocket);

impl DNSBuilder {
    pub fn build(self, pool: AnyPool, runtime: &Runtime) -> DNS {
        let root = LowerName::from(Name::root());
        let mut catalog = Catalog::new();
        catalog.upsert(root, Box::new(DatabaseAuthority::new(pool)));
        let mut server = ServerFuture::new(catalog);
        server.register_socket(self.0, runtime);

        DNS { server }
    }
}
