use anyhow::Error;
use std::any::Any;
use std::fmt::{Debug, Display};
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

// E allows all StdError + Send + Sync + 'static or AnyhowError as this can only be constructed from the former
pub(crate) fn error<I, E>(err: I) -> E
where
    I: Into<Error> + 'static,
    E: From<IoError> + Display + Debug + Send + Sync + 'static,
{
    // specialization if error is E or IoError
    // gets optimized away at compile time
    let mut err = Some(err);
    if let Some(err) = Any::downcast_mut::<Option<E>>(&mut err) {
        return err.take().unwrap();
    }
    if let Some(err) = Any::downcast_mut::<Option<IoError>>(&mut err) {
        return err.take().unwrap().into();
    }

    let err = err.unwrap().into();

    let err = match err.downcast::<E>() {
        Ok(err) => return err,
        Err(err) => err,
    };

    let err = match err.downcast::<IoError>() {
        Ok(err) => err,
        Err(err) => IoError::new(ErrorKind::Other, err),
    };

    err.into()
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
    #[should_panic]
    fn should_panic_extract_error() {
        extract_error(acme_lib::Error::Other("Test".to_owned()));
    }

    fn extract_error(err: acme_lib::Error) -> IoError {
        match err {
            acme_lib::Error::Io(err) => err,
            _ => panic!("Cannot match err"),
        }
    }

    #[test]
    fn io_error_works() {
        let expected = IoError::new(ErrorKind::InvalidData, "Hallo");

        let actual = IoError::new(ErrorKind::InvalidData, "Hallo");
        let actual = extract_error(error(actual));

        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
    }

    #[test]
    fn error_works() {
        let expected = anyhow!("test");

        let actual = extract_error(error(anyhow!("test")));

        assert_eq!(ErrorKind::Other, actual.kind());

        // is not equal because original error gets boxed in an io error
        assert_ne!(format!("{:?}", expected), format!("{:?}", actual));

        // here we access the actual inner error
        let actual = actual.into_inner().expect("Error has no inner error");
        assert_eq!(format!("{}", expected), format!("{}", actual));
    }
}
