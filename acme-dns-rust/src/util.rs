use std::marker::PhantomData;
use std::thread::JoinHandle;

type PhantomNotSendSync = PhantomData<*const ()>;

#[derive(Default)]
struct ParkCalledTokenInner(PhantomNotSendSync);

/// Is !Send and !Sync
/// otherwise the code below would compile
/// this would make it possible to get a ParkCalledToken without the Thread being parked
/// ```compile_fail
/// use acme_dns_rust::util::UnsyncRAIIRef;
///
/// UnsyncRAIIRef::new(|parker| {
///     // spawn an inner Thread and move the parker into it
///     let inner = std::thread::spawn(move || {
///         parker.park()
///     });
///     // unpark inner thread
///     inner.thread().unpark();
///     // receive ParkerCalledToken from joining the inner thread
///     let park_called_token = inner.join().unwrap();
///     // now we have the park_called_token without the thread being parked
///     park_called_token
/// });
/// ```
pub struct ParkCalledToken(ParkCalledTokenInner);

pub struct Parker;

impl Parker {
    pub fn park(self) -> ParkCalledToken {
        std::thread::park();
        ParkCalledToken(ParkCalledTokenInner::default())
    }
}

pub struct UnsyncRAIIRef {
    handle: Option<JoinHandle<()>>,
}

impl UnsyncRAIIRef {
    pub fn new<T>(initializer: T) -> Self
    where
        T: FnOnce(Parker) -> ParkCalledToken + Send + 'static,
    {
        let handle = std::thread::spawn(move || {
            initializer(Parker);
        });

        Self {
            handle: Some(handle),
        }
    }
}

impl Drop for UnsyncRAIIRef {
    fn drop(&mut self) {
        let handle = self.handle.take().unwrap();
        handle.thread().unpark();
        handle.join().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::*;

    #[test]
    fn test_unsync() {
        struct NotSendAndSync {
            _rc: Rc<()>,
            has_dropped: Arc<AtomicBool>,
        }

        impl Drop for NotSendAndSync {
            fn drop(&mut self) {
                self.has_dropped.store(true, Ordering::Relaxed);
            }
        }

        let has_dropped = Arc::new(AtomicBool::new(false));
        let has_dropped_clone = has_dropped.clone();

        let handle = UnsyncRAIIRef::new(move |parker| {
            let _not_send_and_sync = NotSendAndSync {
                _rc: Rc::new(()),
                has_dropped,
            };
            parker.park()
        });

        let has_dropped = has_dropped_clone;
        assert!(!has_dropped.load(Ordering::Relaxed));

        handle_is_sync_and_send(handle);
        assert!(has_dropped.load(Ordering::Relaxed));
    }

    fn handle_is_sync_and_send<T: Sync + Send>(_handle: T) {}
}
