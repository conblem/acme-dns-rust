use acme_dns_rust::run;
use tracing::error;

fn main() {
    tracing_subscriber::fmt::init();

    if let Err(e) = run() {
        error!("{:?}", e);
        std::process::exit(1);
    }
}
