use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer};
use trust_dns_server::resolver::config::{NameServerConfigGroup, ResolverConfig};

pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<ResolverConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let group = match Option::<&str>::deserialize(deserializer)? {
        Some("cloudflare") => NameServerConfigGroup::cloudflare(),
        Some("cloudflare_https") => NameServerConfigGroup::cloudflare_https(),
        Some("cloudflare_tls") => NameServerConfigGroup::cloudflare_tls(),
        Some(tls) if tls.starts_with("tls://") => unreachable!(),
        Some(https) if https.starts_with("https://") => unreachable!(),
        Some(res) => {
            let ip = res.parse().map_err(DeError::custom)?;
            NameServerConfigGroup::from_ips_clear(&[ip], 53, false)
        }
        None => return Ok(None),
    };

    Ok(Some(ResolverConfig::from_parts(None, vec![], group)))
}
