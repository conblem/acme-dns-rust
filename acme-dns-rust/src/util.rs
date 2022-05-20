use anyhow::Error;
use std::any::Any;
use std::error::Error as StdError;
use std::fmt::{Debug, Display};
use std::io::{Error as IoError, ErrorKind};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const fn to_i64(val: u64) -> i64 {
    i64::from_ne_bytes(val.to_ne_bytes())
}
pub const fn to_u64(val: i64) -> u64 {
    u64::from_ne_bytes(val.to_ne_bytes())
}

pub(crate) const HOUR_IN_SECONDS: u64 = 3600;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("unix epoch is safe")
        .as_secs()
}

// this function is used to downcast error into different types
// it takes anything that can be turned into a boxed error
// this includes boxed errors, errors implementing std::error::Error
// or for example strings
// afterwards it tries to turn them into E using the least amount of overhead
// all <dyn Any>::downcast_mut are evaluated at compile time and get removed
// also wrapping the error in an option gets removed as well as it can be statically proven to always be some
pub(crate) fn error<I, E>(err: I) -> E
where
    I: Into<Box<dyn StdError + Send + Sync>> + 'static,
    E: From<IoError> + Display + Debug + Send + Sync + 'static,
{
    let mut err = Some(err);

    // the input and output error are the same do nothing
    if let Some(err) = <dyn Any>::downcast_mut::<Option<E>>(&mut err) {
        return err.take().unwrap();
    }

    // the input error is an IoError use From<IoError> of E to turn it into E
    if let Some(err) = <dyn Any>::downcast_mut::<Option<IoError>>(&mut err) {
        return err.take().unwrap().into();
    }

    // The error is an anyhow error so we try to downcast the internal anyhow error first
    if let Some(err) = <dyn Any>::downcast_mut::<Option<Error>>(&mut err) {
        // use to the anyhow downcast function to downcast directly to E
        let err = match err.take().unwrap().downcast::<E>() {
            Ok(err) => return err,
            Err(err) => err,
        };

        // try to downcast the anyhow error into an IoError, same as before
        let err = match err.downcast::<IoError>() {
            Ok(err) => err,
            Err(err) => IoError::new(ErrorKind::Other, err),
        };
        return err.into();
    }

    // if all else fails use IoError::new which takes anything that can be turned into a boxed error
    IoError::new(ErrorKind::Other, err.unwrap()).into()
}

pub(crate) fn uuid() -> String {
    Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use anyhow::{anyhow, Error};
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
        let res = to_i64(expected);
        let actual = to_u64(res);
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
    fn self_error_works() {
        let expected = acme_lib::Error::Other("Test".to_owned());

        let actual = acme_lib::Error::Other("Test".to_owned());
        let actual: acme_lib::Error = error(actual);

        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
        assert_eq!(format!("{}", expected), format!("{}", actual));
    }

    #[test]
    fn io_error_works() {
        let expected = IoError::new(ErrorKind::InvalidData, "Hallo");

        let actual = IoError::new(ErrorKind::InvalidData, "Hallo");
        let actual = extract_error(error(actual));

        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
        assert_eq!(format!("{}", expected), format!("{}", actual));
    }

    #[test]
    fn anyhow_works() {
        let expected = acme_lib::Error::Other("Test".to_owned());

        let actual = Error::new(acme_lib::Error::Other("Test".to_owned()));
        let actual: acme_lib::Error = error(actual);

        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
        assert_eq!(format!("{}", expected), format!("{}", actual));
    }

    #[test]
    fn anyhow_io_works() {
        let expected = IoError::from(ErrorKind::Other);

        let actual = Error::new(IoError::from(ErrorKind::Other));
        let actual = extract_error(error(actual));

        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
        assert_eq!(format!("{}", expected), format!("{}", actual));
    }

    #[test]
    fn anyhow_error_works() {
        let expected = IoError::new(ErrorKind::Other, anyhow!("Test"));

        let actual = extract_error(error(anyhow!("Test")));

        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
        assert_eq!(format!("{}", expected), format!("{}", actual));
    }

    #[test]
    fn error_works() {
        let expected = IoError::new(ErrorKind::Other, "Test");

        let actual = extract_error(error("Test"));

        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
        assert_eq!(format!("{}", expected), format!("{}", actual));
    }
}
