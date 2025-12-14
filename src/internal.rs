use alloc::{boxed::Box, collections::VecDeque};

use xutex::AsyncMutex;

use crate::signal::{Signal, SignalTerimnator};

/// Internal of the channel that holds queues, waitlists, and general state of
/// the channel, it's shared among senders and receivers with an atomic
/// counter and a mutex
pub(crate) struct ChannelInternal<T, const CAPACITY: usize> {
    // KEEP THE ORDER
    /// Channel queue to save buffered objects
    pub(crate) queue: VecDeque<T>,
    /// It's true if the signals in the waiting list are recv signals
    pub(crate) recv_blocking: bool,
    /// Receive and Send waitlist for when the channel queue is empty or zero
    /// capacity for recv or full for send.
    pub(crate) wait_list: VecDeque<SignalTerimnator>,
    /// Count of alive receivers
    pub(crate) recv_count: usize,
    /// Count of alive senders
    pub(crate) send_count: usize,
    /// Reference counter for the alive instances of the channel
    pub(crate) ref_count: usize,
}

unsafe impl<T: Send, const CAPACITY: usize> Send for ChannelInternal<T, CAPACITY> {}
unsafe impl<T: Send, const CAPACITY: usize> Sync for ChannelInternal<T, CAPACITY> {}

pub(crate) struct Internal<T, const CAPACITY: usize> {
    pub(crate) inner: *mut AsyncMutex<ChannelInternal<T, CAPACITY>>,
}

impl<T, const CAPACITY: usize> Clone for Internal<T, CAPACITY> {
    fn clone(&self) -> Self {
        Self { inner: self.inner }
    }
}

impl<T, const CAPACITY: usize> Internal<T, CAPACITY> {
    pub(crate) fn new() -> Self {
        Self {
            inner: Box::leak(Box::new(AsyncMutex::new(ChannelInternal {
                queue: VecDeque::with_capacity(CAPACITY),
                recv_blocking: false,
                wait_list: VecDeque::new(),
                recv_count: 1,
                send_count: 1,
                ref_count: 2,
            }))),
        }
    }
    pub(crate) async fn lock(&self) -> xutex::MutexGuard<'_, ChannelInternal<T, CAPACITY>> {
        // SAFETY: it's safe to deref the pointer as long as the internal is
        // alive, which is guaranteed by the ref counting mechanism
        unsafe { (&*self.inner).lock().await }
    }
    pub(crate) fn lock_sync(&self) -> xutex::MutexGuard<'_, ChannelInternal<T, CAPACITY>> {
        // SAFETY: it's safe to deref the pointer as long as the internal is
        // alive, which is guaranteed by the ref counting mechanism
        unsafe { (&*self.inner).as_sync().lock() }
    }
    pub(crate) fn try_lock(&self) -> Option<xutex::MutexGuard<'_, ChannelInternal<T, CAPACITY>>> {
        // SAFETY: it's safe to deref the pointer as long as the internal is
        // alive, which is guaranteed by the ref counting mechanism
        unsafe { (&*self.inner).try_lock() }
    }
}

unsafe impl<T: Send, const CAPACITY: usize> Send for Internal<T, CAPACITY> {}
unsafe impl<T: Send, const CAPACITY: usize> Sync for Internal<T, CAPACITY> {}

impl<T, const CAPACITY: usize> ChannelInternal<T, CAPACITY> {
    /// Terminates remainings signals in the queue to notify listeners about the
    /// closing of the channel
    #[cold]
    pub(crate) fn terminate_signals(&mut self) {
        for t in self.wait_list.iter() {
            // SAFETY: it's safe to terminate owned signal once
            unsafe { t.terminate::<T, CAPACITY>() }
        }
        self.wait_list.clear();
    }

    /// Returns next signal for sender from the waitlist
    #[inline(always)]
    pub(crate) fn next_send(&mut self) -> Option<SignalTerimnator> {
        if self.recv_blocking {
            return None;
        }
        match self.wait_list.pop_front() {
            Some(sig) => Some(sig),
            None => {
                self.recv_blocking = true;
                None
            }
        }
    }

    /// Adds new sender/receiver signal to the waitlist
    #[inline(always)]
    pub(crate) fn push_signal(&mut self, s: SignalTerimnator) {
        self.wait_list.push_back(s);
    }

    /// Returns the next signal for the receiver in the waitlist
    #[inline(always)]
    pub(crate) fn next_recv(&mut self) -> Option<SignalTerimnator> {
        if !self.recv_blocking {
            return None;
        }
        match self.wait_list.pop_front() {
            Some(sig) => Some(sig),
            None => {
                self.recv_blocking = false;
                None
            }
        }
    }

    /// Tries to remove the send signal from the waitlist, returns true if the
    /// operation was successful
    pub(crate) fn cancel_send_signal(&mut self, sig: *const Signal<T, CAPACITY>) -> bool {
        if !self.recv_blocking {
            for (i, send) in self.wait_list.iter().enumerate() {
                if send.eq_ptr(sig as *const ()) {
                    // SAFETY: it's safe to cancel owned signal once, we are sure that index is
                    // valid
                    unsafe {
                        self.wait_list
                            .remove(i)
                            .unwrap_unchecked()
                            .cancel::<T, CAPACITY>();
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Tries to remove the received signal from the waitlist, returns true if
    /// the operation was successful
    pub(crate) fn cancel_recv_signal(&mut self, sig: *const Signal<T, CAPACITY>) -> bool {
        if self.recv_blocking {
            for (i, recv) in self.wait_list.iter().enumerate() {
                if recv.eq_ptr(sig as *const ()) {
                    // SAFETY: it's safe to cancel owned signal once, we are sure that index is
                    // valid
                    unsafe {
                        self.wait_list
                            .remove(i)
                            .unwrap_unchecked()
                            .cancel::<T, CAPACITY>();
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Increases ref count for sender or receiver
    #[inline(always)]
    pub(crate) fn inc_ref_count(&mut self, is_sender: bool) {
        if is_sender {
            if self.send_count > 0 {
                self.send_count += 1;
            }
        } else if self.recv_count > 0 {
            self.recv_count += 1;
        }
        self.ref_count += 1;
    }

    /// Decreases ref count for sender or receiver
    #[inline(always)]
    pub(crate) fn dec_ref_count(&mut self, is_sender: bool) -> bool {
        if is_sender {
            if self.send_count > 0 {
                self.send_count -= 1;
                if self.send_count == 0 && self.recv_count != 0 {
                    self.terminate_signals();
                }
            }
        } else if self.recv_count > 0 {
            self.recv_count -= 1;
            if self.recv_count == 0 {
                self.terminate_signals();
                if CAPACITY > 0 {
                    self.queue.clear();
                }
            }
        }
        self.ref_count -= 1;
        self.ref_count == 0
    }
}
