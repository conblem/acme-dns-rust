use std::time::{SystemTime, UNIX_EPOCH};
use std::io;

pub(crate) const fn to_i64(val: &u64) -> i64 {
    i64::from_ne_bytes(val.to_ne_bytes())
}

pub(crate) const HOUR: u64 = 3600;
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("unix epoch is safe")
        .as_secs()
}

pub(crate) fn error<E: From<io::Error>>(err: impl Into<anyhow::Error>) -> E {
    let err = err.into();
    let err = match err.downcast::<io::Error>() {
        Ok(err) => err,
        Err(err) => io::Error::new(io::ErrorKind::Other, err),
    };

    E::from(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    const NUMBER_1: u64 = 2323;
    const NUMBER_2: u64 = 940329402394;
    #[test]
    fn to_i64_works() {
        test_to_i64(NUMBER_1);
        test_to_i64(NUMBER_2);
        test_to_i64(u64::MAX);
        test_to_i64(u64::MIN);
    }

    fn test_to_i64(expected: u64) {
        let res = to_i64(&expected);
        let actual = u64::from_ne_bytes(res.to_ne_bytes());
        assert_eq!(expected, actual)
    }

    #[test]
    fn now_works() {
        let actual = now();
        thread::sleep(Duration::from_millis(1500));
        assert_ne!(actual, now())
    }
}
