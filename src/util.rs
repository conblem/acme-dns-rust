use anyhow::Error;
use std::io::{Error as IoError, ErrorKind};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const fn to_i64(val: &u64) -> i64 {
    i64::from_ne_bytes(val.to_ne_bytes())
}
pub const fn to_u64(val: &i64) -> u64 {
    u64::from_ne_bytes(val.to_ne_bytes())
}

pub(crate) const HOUR: u64 = 3600;
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("unix epoch is safe")
        .as_secs()
}

pub(crate) fn error<I: Into<Error>, E: From<IoError>>(err: I) -> E {
    let err = err.into();
    let err = match err.downcast::<IoError>() {
        Ok(err) => err,
        Err(err) => IoError::new(ErrorKind::Other, err),
    };

    E::from(err)
}

pub(crate) fn uuid() -> String {
    Uuid::new_v4().to_simple().to_string()
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use std::io::{Error as IoError, ErrorKind};
    use std::thread;
    use std::time::Duration;

    use super::{error, now, to_i64, to_u64, uuid};

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
        let actual = to_u64(&res);
        assert_eq!(expected, actual)
    }

    #[test]
    fn uuid_test() {
        assert_ne!(uuid(), uuid());

        let len = uuid().len();
        assert_eq!(32, len)
    }

    #[test]
    fn now_works() {
        let actual = now();
        thread::sleep(Duration::from_millis(1500));
        assert!(actual < now())
    }

    #[test]
    fn io_error_works() {
        let expected = IoError::new(ErrorKind::InvalidData, "Hallo");
        let err = IoError::new(ErrorKind::InvalidData, "Hallo");

        let actual = match error(err) {
            acme_lib::Error::Io(err) => err,
            _ => panic!("Cannot match err"),
        };

        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
    }

    #[test]
    fn error_works() {
        let expected = anyhow!("test");

        let actual = match error(anyhow!("test")) {
            acme_lib::Error::Io(err) => err,
            _ => panic!("Cannot match err"),
        };

        assert_eq!(ErrorKind::Other, actual.kind());

        // is not equal because original error gets boxed in an io error
        assert_ne!(format!("{:?}", expected), format!("{:?}", actual));

        // here we access the actual inner error
        let actual = actual.into_inner().expect("Error has no inner error");
        assert_eq!(format!("{}", expected), format!("{}", actual));
    }
}
