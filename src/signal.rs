use core::{
    cell::UnsafeCell,
    future::Future,
    mem::MaybeUninit,
    pin,
    sync::atomic::{fence, AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
    time::Duration,
};

use branches::{likely, unlikely};
use xutex::{AsyncMutex, Mutex};

use crate::{
    backoff::*,
    internal::{ChannelInternal, Internal},
    ReceiveError, SendError,
};

const UNINITED: usize = 0;
const PENDING: usize = UNINITED + 1;
const TERMINATED: usize = usize::MAX - 1;
const SIGNALLED: usize = usize::MAX;

enum OnceWakerInner {
    Uninited,
    // When notified it never updates again
    Notified,
    Waker(Waker),
}

pub(crate) struct OnceWaker(Mutex<OnceWakerInner>);

impl OnceWaker {
    fn new() -> Self {
        Self(Mutex::new(OnceWakerInner::Uninited))
    }
    // returns true if waker is not notified
    pub(crate) fn update(&self, other: &Waker) -> bool {
        let this = &mut *self.0.lock();
        match this {
            OnceWakerInner::Waker(waker) => {
                if !waker.will_wake(other) {
                    *waker = other.clone();
                }
                true
            }
            OnceWakerInner::Notified => false,
            OnceWakerInner::Uninited => {
                *this = OnceWakerInner::Waker(other.clone());
                true
            }
        }
    }

    pub(crate) fn take(&self) -> Option<Waker> {
        let this = &mut *self.0.lock();
        match core::mem::replace(this, OnceWakerInner::Notified) {
            OnceWakerInner::Waker(waker) => Some(waker),
            _ => None,
        }
    }
}

pub(crate) struct Signal<'a, T, const CAPACITY: usize> {
    state: AtomicUsize,
    data: UnsafeCell<MaybeUninit<T>>,
    waker: OnceWaker,
    internal: &'a AsyncMutex<ChannelInternal<T, CAPACITY>>,
    _pinned: core::marker::PhantomPinned,
}

pub(crate) struct ReceiveSignal<'a, T, const CAPACITY: usize> {
    inner: Signal<'a, T, CAPACITY>,
}

impl<'a, T, const CAPACITY: usize> ReceiveSignal<'a, T, CAPACITY> {
    pub(crate) fn new(internal: &'a Internal<T, CAPACITY>) -> Self {
        Signal::new_recv(internal)
    }
    pub(crate) fn get_terminator(&self) -> SignalTerimnator {
        self.inner.get_terminator()
    }
}

impl<'a, T, const CAPACITY: usize> Drop for ReceiveSignal<'a, T, CAPACITY> {
    fn drop(&mut self) {
        if unlikely(self.inner.result() == SignalResult::Pending) {
            let state = self.inner.state.load(Ordering::Acquire);
            if state == PENDING {
                #[cfg(feature = "std")]
                #[cfg(debug_assertions)]
                {
                    eprintln!("Warning: ReceiveSignal dropped before its finish state, data loss happened, do not drop ReceiveSignal before it is resolved");
                }
                let mut internal = self.inner.internal.lock_sync();
                if !internal.cancel_recv_signal(self.inner.as_ptr()) {
                    drop(internal);
                    // a sender got signal ownership, should wait until the response
                    if self.inner.blocking_wait() {
                        // got ownership of data that is not going to be used ever again, so drop it
                        // this is actually a bug in user code but we should handle it gracefully
                        // and we warn user in debug mode
                        // SAFETY: data is not moved it's safe to drop it or put it back to the
                        // channel queue
                        if CAPACITY == 0 {
                            unsafe {
                                self.inner.drop_data();
                            }
                        } else {
                            // This might grow the queue size over capacity, but it's here to help
                            // correctness of users program until they
                            // fix their bug
                            self.inner
                                .internal
                                .lock_sync()
                                .queue
                                .push_back(unsafe { self.inner.assume_init() });
                        }
                    }
                } else {
                    drop(internal);
                }
            }
        }
    }
}

impl<'a, T, const CAPACITY: usize> Future for ReceiveSignal<'a, T, CAPACITY> {
    type Output = Result<T, ReceiveError>;
    fn poll(self: pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        match this.inner.result() {
            SignalResult::Uninited => {
                if this.inner.waker.update(cx.waker()) {
                    if let Err(current_state) = this.inner.state.compare_exchange(
                        UNINITED,
                        PENDING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        if current_state == SIGNALLED {
                            let data = unsafe { Signal::read_data(&this.inner) };
                            return Poll::Ready(Ok(data));
                        } else if current_state == TERMINATED {
                            return Poll::Ready(Err(ReceiveError()));
                        }
                    }
                    Poll::Pending
                } else {
                    // we were already notified, try to wait synchronously
                    if this.inner.blocking_wait() {
                        let data = unsafe { Signal::read_data(&this.inner) };
                        return Poll::Ready(Ok(data));
                    }
                    // not ready, return error
                    Poll::Ready(Err(ReceiveError()))
                }
            }
            SignalResult::Pending => {
                if this.inner.waker.update(cx.waker()) {
                    // Waker was still set, just return Pending
                    return Poll::Pending;
                }
                // Waker was None. We are getting updated, wait for signal sync.
                if this.inner.blocking_wait() {
                    let data = unsafe { Signal::read_data(&this.inner) };
                    return Poll::Ready(Ok(data));
                }
                // Not ready, return the error.
                Poll::Ready(Err(ReceiveError()))
            }
            SignalResult::Success => {
                let data = unsafe { Signal::read_data(&this.inner) };
                Poll::Ready(Ok(data))
            }
            SignalResult::Failure => Poll::Ready(Err(ReceiveError())),
        }
    }
}

pub(crate) struct SendSignal<'a, T, const CAPACITY: usize> {
    inner: Signal<'a, T, CAPACITY>,
}

impl<'a, T, const CAPACITY: usize> SendSignal<'a, T, CAPACITY> {
    pub(crate) fn new(internal: &'a Internal<T, CAPACITY>, data: T) -> Self {
        Signal::new_send(internal, data)
    }
    pub(crate) fn get_terminator(&self) -> SignalTerimnator {
        self.inner.get_terminator()
    }
}

impl<'a, T, const CAPACITY: usize> Drop for SendSignal<'a, T, CAPACITY> {
    fn drop(&mut self) {
        if unlikely(self.inner.result() == SignalResult::Pending) {
            let state = self.inner.state.load(Ordering::Acquire);
            if state == PENDING {
                let mut internal = self.inner.internal.lock_sync();
                if !internal.cancel_send_signal(self.inner.as_ptr()) {
                    drop(internal);
                    // a receiver got signal ownership, should wait until the response
                    if self.inner.blocking_wait() {
                        // no need to drop data, it was moved to receiver
                        return;
                    }
                } else {
                    drop(internal);
                }
            }
            // signal is canceled, or in uninited state, drop data locally
            // SAFETY: data is not moved, it's safe to drop it
            unsafe {
                self.inner.drop_data();
            }
        }
    }
}
impl<'a, T, const CAPACITY: usize> Future for SendSignal<'a, T, CAPACITY> {
    type Output = Result<(), SendError<T>>;
    fn poll(self: pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        match this.inner.result() {
            SignalResult::Uninited => {
                if this.inner.waker.update(cx.waker()) {
                    if let Err(current_state) = this.inner.state.compare_exchange(
                        UNINITED,
                        PENDING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        if current_state == SIGNALLED {
                            return Poll::Ready(Ok(()));
                        } else if current_state == TERMINATED {
                            // SAFETY: data failed to move so we return it to user
                            return Poll::Ready(Err(SendError(unsafe {
                                this.inner.assume_init()
                            })));
                        }
                    }
                    Poll::Pending
                } else {
                    // we were already notified, try to wait synchronously
                    if this.inner.blocking_wait() {
                        return Poll::Ready(Ok(()));
                    }
                    // ready return error with data
                    // SAFETY: data failed to move so we return it to user
                    Poll::Ready(Err(SendError(unsafe { this.inner.assume_init() })))
                }
            }
            SignalResult::Pending => {
                if this.inner.waker.update(cx.waker()) {
                    // Waker was still set, just return Pending
                    return Poll::Pending;
                }
                // Waker was None. We are getting updated, wait for signal sync.
                if this.inner.blocking_wait() {
                    return Poll::Ready(Ok(()));
                }
                // Not ready, return the error with data.
                // SAFETY: data failed to move so we return it to user
                Poll::Ready(Err(SendError(unsafe { this.inner.assume_init() })))
            }
            SignalResult::Success => Poll::Ready(Ok(())),
            SignalResult::Failure => {
                // SAFETY: data failed to move so we return it to user
                Poll::Ready(Err(SendError(unsafe { this.inner.assume_init() })))
            }
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum SignalResult {
    Uninited,
    Pending,
    Success,
    Failure,
}

impl<'a, T, const CAPACITY: usize> Signal<'a, T, CAPACITY> {
    #[inline(always)]
    fn new_recv(internal: &'a Internal<T, CAPACITY>) -> ReceiveSignal<'a, T, CAPACITY> {
        ReceiveSignal {
            inner: Self {
                state: AtomicUsize::new(UNINITED),
                data: UnsafeCell::new(MaybeUninit::uninit()),
                waker: OnceWaker::new(),
                internal: unsafe { &*internal.inner },
                _pinned: core::marker::PhantomPinned,
            },
        }
    }
    #[inline(always)]
    fn new_send(internal: &'a Internal<T, CAPACITY>, data: T) -> SendSignal<'a, T, CAPACITY> {
        SendSignal {
            inner: Self {
                state: AtomicUsize::new(UNINITED),
                data: UnsafeCell::new(MaybeUninit::new(data)),
                waker: OnceWaker::new(),
                internal: unsafe { &*internal.inner },
                _pinned: core::marker::PhantomPinned,
            },
        }
    }

    pub(crate) fn as_ptr(&'a self) -> *const Signal<'a, T, CAPACITY> {
        self as *const Self
    }

    #[inline(always)]
    pub(crate) fn result(&self) -> SignalResult {
        let v = self.state.load(Ordering::Acquire);
        if likely(v == SIGNALLED) {
            SignalResult::Success
        } else if v == TERMINATED {
            SignalResult::Failure
        } else if v == PENDING {
            SignalResult::Pending
        } else {
            SignalResult::Uninited
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn write_data(this: *const Self, data: T) {
        let waker = (*this).waker.take();
        (*this).data.get().write(MaybeUninit::new(data));
        (*this).state.store(SIGNALLED, Ordering::Release);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn read_data(this: *const Self) -> T {
        let waker = (*this).waker.take();
        let data = (*this).data.get().read().assume_init();
        (*this).state.store(SIGNALLED, Ordering::Release);
        if let Some(waker) = waker {
            waker.wake();
        }
        data
    }
    // Drops waker without waking
    pub(crate) unsafe fn cancel(this: *const Self) {
        let waker = (*this).waker.take();
        (*this).state.store(TERMINATED, Ordering::Release);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
    pub(crate) unsafe fn assume_init(&self) -> T {
        unsafe { self.data.get().read().assume_init() }
    }
    pub(crate) unsafe fn terminate(this: *const Self) {
        let waker = (*this).waker.take();
        (*this).state.store(TERMINATED, Ordering::Release);
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    #[inline(always)]
    pub(crate) unsafe fn drop_data(&mut self) {
        let ptr = self.data.get();
        (&mut *ptr).as_mut_ptr().drop_in_place();
    }
    #[inline(never)]
    #[cold]
    pub(crate) fn blocking_wait(&self) -> bool {
        let v = self.state.load(Ordering::Relaxed);
        if likely(v > PENDING) {
            fence(Ordering::Acquire);
            return v == SIGNALLED;
        }

        for _ in 0..32 {
            yield_now();
            let v = self.state.load(Ordering::Relaxed);
            if likely(v > PENDING) {
                fence(Ordering::Acquire);
                return v == SIGNALLED;
            }
        }

        // Usually this part will not happen but you can't be sure
        let mut sleep_time: u64 = 1 << 10;
        loop {
            sleep(Duration::from_nanos(sleep_time));
            let v = self.state.load(Ordering::Relaxed);
            if likely(v > PENDING) {
                fence(Ordering::Acquire);
                return v == SIGNALLED;
            }
            // increase sleep_time gradually to 262 microseconds
            if sleep_time < (1 << 18) {
                sleep_time <<= 1;
            }
        }
    }

    pub(crate) fn get_terminator(&self) -> SignalTerimnator {
        SignalTerimnator {
            inner: self as *const Self as *const (),
        }
    }
}

pub(crate) struct SignalTerimnator {
    inner: *const (),
}

impl SignalTerimnator {
    pub(crate) unsafe fn terminate<T, const CAPACITY: usize>(&self) {
        Signal::terminate(self.inner as *const Signal<T, CAPACITY>);
    }
    pub(crate) unsafe fn send<T, const CAPACITY: usize>(self, data: T) {
        Signal::write_data(self.inner as *const Signal<T, CAPACITY>, data);
    }
    pub(crate) unsafe fn recv<T, const CAPACITY: usize>(self) -> T {
        Signal::read_data(self.inner as *const Signal<T, CAPACITY>)
    }
    pub(crate) unsafe fn cancel<T, const CAPACITY: usize>(self) {
        Signal::cancel(self.inner as *const Signal<T, CAPACITY>);
    }
    pub(crate) fn eq_ptr(&self, other: *const ()) -> bool {
        core::ptr::eq(self.inner, other)
    }
}

unsafe impl<'a, T: Send, const CAPACITY: usize> Send for Signal<'a, T, CAPACITY> {}
unsafe impl<'a, T: Send, const CAPACITY: usize> Sync for Signal<'a, T, CAPACITY> {}
