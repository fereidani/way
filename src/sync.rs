use branches::unlikely;
use swait::*;

use crate::{
    error::{CloseError, ReceiveError, SendError},
    internal::Internal,
    ReceiveErrorTimeout, Receiver, Sender,
};

#[repr(C)]
pub struct SyncSender<T, const CAPACITY: usize = 0> {
    internal: Internal<T, CAPACITY>,
}

macro_rules! shared_impl {
    () => {
        pub fn len(&self) -> usize {
            self.internal.lock_sync().queue.len()
        }

        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }

        pub fn is_full(&self) -> bool {
            if CAPACITY == 0 {
                return true;
            }
            CAPACITY == self.internal.lock_sync().queue.len()
        }

        pub fn capacity(&self) -> usize {
            CAPACITY
        }

        pub fn receiver_count(&self) -> usize {
            self.internal.lock_sync().recv_count
        }

        pub fn sender_count(&self) -> usize {
            self.internal.lock_sync().send_count
        }

        pub fn close(&self) -> Result<(), CloseError> {
            let mut internal = self.internal.lock_sync();
            if unlikely(internal.recv_count == 0 && internal.send_count == 0) {
                return Err(CloseError());
            }
            internal.recv_count = 0;
            internal.send_count = 0;
            internal.terminate_signals();
            internal.queue.clear();
            Ok(())
        }

        pub fn is_closed(&self) -> bool {
            let internal = self.internal.lock_sync();
            internal.send_count == 0 && internal.recv_count == 0
        }
    };
}

impl<T, const CAPACITY: usize> SyncSender<T, CAPACITY> {
    shared_impl!();
    pub fn as_async(&self) -> &Sender<T, CAPACITY> {
        // SAFETY: SyncSender and Sender have the same layout
        unsafe { core::mem::transmute(self) }
    }
    pub fn to_async(self) -> Sender<T, CAPACITY> {
        // SAFETY: SyncSender and Sender have the same layout
        unsafe { core::mem::transmute(self) }
    }
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.as_async().send(value).swait()
    }
    pub fn try_send(&self, data: T) -> Result<(), SendError<T>> {
        self.as_async().try_send(data).swait()
    }

    pub fn try_send_realtime(&self, data: T) -> Result<(), SendError<T>> {
        self.as_async().try_send_realtime(data)
    }
    pub fn is_disconnected(&self) -> bool {
        self.as_async().is_disconnected().swait()
    }
}

impl<T, const CAPACITY: usize> Clone for SyncSender<T, CAPACITY> {
    fn clone(&self) -> Self {
        self.internal.lock_sync().inc_ref_count(true);
        SyncSender {
            internal: self.internal.clone(),
        }
    }
}

impl<T, const CAPACITY: usize> Drop for SyncSender<T, CAPACITY> {
    fn drop(&mut self) {
        let mut guard = self.internal.lock_sync();
        if guard.dec_ref_count(true) {
            // SAFETY: this was the last refrence to the internal channel, it's safe to
            // deallocate it now
            drop(guard);
            unsafe {
                _ = Box::from_raw(self.internal.inner);
            }
        }
    }
}

#[repr(C)]
pub struct SyncReceiver<T, const CAPACITY: usize = 0> {
    internal: Internal<T, CAPACITY>,
}

impl<T, const CAPACITY: usize> SyncReceiver<T, CAPACITY> {
    shared_impl!();
    pub fn as_async(&self) -> &Receiver<T, CAPACITY> {
        // SAFETY: SyncReceiver and Receiver have the same layout
        unsafe { &*(self as *const SyncReceiver<T, CAPACITY> as *const Receiver<T, CAPACITY>) }
    }
    pub fn to_async(self) -> Sender<T, CAPACITY> {
        // SAFETY: SyncReceiver and Receiver have the same layout
        unsafe { core::mem::transmute(self) }
    }
    pub fn recv(&self) -> Result<T, ReceiveError> {
        self.as_async().recv().swait()
    }

    pub fn try_recv(&self) -> Result<T, ReceiveErrorTimeout> {
        self.as_async().try_recv().swait()
    }

    pub fn try_recv_realtime(&self) -> Result<T, ReceiveErrorTimeout> {
        self.as_async().try_recv_realtime()
    }

    pub fn is_disconnected(&self) -> bool {
        self.as_async().is_disconnected().swait()
    }
}

impl<T, const CAPACITY: usize> Clone for SyncReceiver<T, CAPACITY> {
    fn clone(&self) -> Self {
        self.internal.lock_sync().inc_ref_count(false);
        SyncReceiver {
            internal: self.internal.clone(),
        }
    }
}

impl<T, const CAPACITY: usize> Drop for SyncReceiver<T, CAPACITY> {
    fn drop(&mut self) {
        let mut guard = self.internal.lock_sync();
        if guard.dec_ref_count(false) {
            // SAFETY: this was the last refrence to the internal channel, it's safe to
            // deallocate it now
            drop(guard);
            unsafe {
                _ = Box::from_raw(self.internal.inner);
            }
        }
    }
}

pub fn bounded_sync<T, const CAPACITY: usize>(
) -> (SyncSender<T, CAPACITY>, SyncReceiver<T, CAPACITY>) {
    let internal = Internal::<T, CAPACITY>::new();
    (
        SyncSender {
            internal: internal.clone(),
        },
        SyncReceiver { internal },
    )
}

pub fn zero_sync<T>() -> (SyncSender<T, 0>, SyncReceiver<T, 0>) {
    bounded_sync::<T, 0>()
}
