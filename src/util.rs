use anyhow::Error;
use std::io::{Error as IoError, ErrorKind};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) const fn to_i64(val: &u64) -> i64 {
    i64::from_ne_bytes(val.to_ne_bytes())
}
pub(crate) const fn to_u64(val: &i64) -> u64 {
    u64::from_ne_bytes(val.to_ne_bytes())
}

pub(crate) const HOUR: u64 = 3600;
pub(crate) fn now() -> u64 {
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

macro_rules! delegate_async_write {
    ($item:ident) => {
        fn poll_write(
            mut self: ::std::pin::Pin<&mut Self>,
            cx: &mut ::std::task::Context<'_>,
            buf: &[::std::primitive::u8],
        ) -> ::std::task::Poll<::std::io::Result<usize>> {
            ::std::pin::Pin::new(&mut self.$item).poll_write(cx, buf)
        }

        fn poll_flush(
            mut self: ::std::pin::Pin<&mut Self>,
            cx: &mut ::std::task::Context<'_>,
        ) -> ::std::task::Poll<::std::io::Result<()>> {
            ::std::pin::Pin::new(&mut self.$item).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: ::std::pin::Pin<&mut Self>,
            cx: &mut ::std::task::Context<'_>,
        ) -> ::std::task::Poll<::std::io::Result<()>> {
            ::std::pin::Pin::new(&mut self.$item).poll_shutdown(cx)
        }

        fn poll_write_vectored(
            mut self: ::std::pin::Pin<&mut Self>,
            cx: &mut ::std::task::Context<'_>,
            bufs: &[::std::io::IoSlice<'_>],
        ) -> ::std::task::Poll<::std::io::Result<::std::primitive::usize>> {
            ::std::pin::Pin::new(&mut self.$item).poll_write_vectored(cx, bufs)
        }

        fn is_write_vectored(&self) -> ::std::primitive::bool {
            self.$item.is_write_vectored()
        }
    };
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use futures_util::future;
    use std::io::{Error as IoError, ErrorKind, IoSlice};
    use std::pin::Pin;
    use std::thread;
    use std::time::Duration;
    use tokio::io::{AsyncWrite, AsyncWriteExt};
    use tokio_test::io::Builder;

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

    struct Wrapper<T> {
        inner: T,
    }

    impl<T: AsyncWrite + Unpin> AsyncWrite for Wrapper<T> {
        delegate_async_write!(inner);
    }

    #[tokio::test]
    async fn delegate_async_write_works() {
        let mut builder = Builder::new();
        builder.write("Test1".as_ref());
        builder.write("Test2".as_ref());

        let mut stream = Wrapper {
            inner: builder.build(),
        };
        assert_eq!(false, stream.is_write_vectored());

        stream.write_all("Test1".as_ref()).await.unwrap();

        let slice = IoSlice::new("Test2".as_ref());
        let size =
            future::poll_fn(move |cx| Pin::new(&mut stream).poll_write_vectored(cx, &[slice]))
                .await
                .unwrap();
        assert_eq!(5, size);

        let mut stream = Wrapper {
            inner: Builder::new().build(),
        };
        assert_eq!((), stream.flush().await.unwrap());
        assert_eq!((), stream.shutdown().await.unwrap());
    }
}
