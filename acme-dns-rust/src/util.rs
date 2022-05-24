use std::marker::PhantomData;
use std::thread::JoinHandle;

/// used to make sure parker do not get swapped with different threads
/// ```compile_fail
/// let _handle = UnsyncRAIIRef::new(|parker| {
///     let inner = std::thread::spawn(move || {
///         parker.park()
///     });
///     inner.unpark();
///     inner.join().unwrap()
/// });
/// ```
//type PhantomNotSendSync = PhantomData<*const ()>;
type PhantomNotSendSync = ();

#[derive(Default)]
struct ParkCalledTokenInner(PhantomNotSendSync);
pub(crate) struct ParkCalledToken(ParkCalledTokenInner);

pub(crate) struct Parker(PhantomNotSendSync);

impl Parker {
    pub(crate) fn park(self) -> ParkCalledToken {
        std::thread::park();
        ParkCalledToken(ParkCalledTokenInner::default())
    }
}

pub(crate) struct UnsyncRAIIRef {
    handle: Option<JoinHandle<()>>,
}

impl UnsyncRAIIRef {
    pub(crate) fn new<T>(initializer: T) -> Self
    where
        T: FnOnce(Parker) -> ParkCalledToken + Send + 'static,
    {
        let handle = std::thread::spawn(move || {
            initializer(Parker(PhantomNotSendSync::default()));
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
