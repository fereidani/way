#![cfg_attr(not(feature = "std"), no_std)]
use alloc::{boxed::Box, vec::Vec};
use core::pin::pin;

use branches::unlikely;
pub use error::*;

extern crate alloc;

mod backoff;
mod error;
mod internal;
mod signal;

#[cfg(feature = "sync")]
pub use sync::*;

#[cfg(feature = "sync")]
mod sync;

pub use crate::error::{ReceiveError, SendError};
use crate::{
    internal::Internal,
    signal::{ReceiveSignal, SendSignal},
};

#[repr(C)]
pub struct Sender<T, const CAPACITY: usize = 0> {
    internal: Internal<T, CAPACITY>,
}

#[repr(C)]
pub struct Receiver<T, const CAPACITY: usize = 0> {
    internal: Internal<T, CAPACITY>,
}

macro_rules! shared_impl {
    () => {
        /// Returns length of the queue.
        ///
        /// # Examples
        ///
        /// ```
        /// # use swait::*;
        /// # use way::*;
        /// # async {
        /// let (s, r) = bounded::<u64,8>();
        /// assert_eq!(s.len().await,0);
        /// assert_eq!(r.len().await,0);
        /// s.send(10).await.unwrap();
        /// assert_eq!(s.len().await,1);
        /// assert_eq!(r.len().await,1);
        /// # }.swait();
        /// ```
        pub async fn len(&self) -> usize {
            if CAPACITY == 0 {
                return 0;
            }
            self.internal.lock().await.queue.len()
        }
        /// Returns whether the channel queue is empty or not.
        ///
        /// # Examples
        ///
        /// ```
        /// # use swait::*;
        /// # use way::*;
        /// # async {
        /// let (s, r) = bounded::<u64,8>();
        /// assert_eq!(s.is_empty().await,true);
        /// assert_eq!(r.is_empty().await,true);
        /// # }.swait();
        /// ```
        pub async fn is_empty(&self) -> bool {
            self.internal.lock().await.queue.is_empty()
        }
        /// Returns whether the channel queue is full or not
        /// full channels will block on send and recv calls
        /// it always returns true for zero sized channels.
        ///
        /// # Examples
        ///
        /// ```
        /// # use swait::*;
        /// # use way::*;
        /// # async {
        /// let (s, r) = bounded::<&str, 1>();
        /// s.send("Hi!").await.unwrap();
        /// assert_eq!(s.is_full().await,true);
        /// assert_eq!(r.is_full().await,true);
        /// # }.swait()
        /// ```
        pub async fn is_full(&self) -> bool {
            if CAPACITY == 0 {
                return true;
            }
            CAPACITY == self.internal.lock().await.queue.len()
        }
        /// Returns capacity of channel (not the queue)
        ///
        /// # Examples
        ///
        /// ```
        /// # use way::*;
        /// let (s, r) = bounded::<u64, 0>();
        /// assert_eq!(s.capacity(),0);
        /// assert_eq!(r.capacity(),0);
        /// ```
        /// ```
        /// # use way::*;
        /// let (s, r) = bounded::<u64,64>();
        /// assert_eq!(s.capacity(),64);
        /// assert_eq!(r.capacity(),64);
        /// ```
        pub fn capacity(&self) -> usize {
            CAPACITY
        }
        /// Returns count of alive receiver instances of the channel.
        ///
        /// # Examples
        ///
        /// ```
        /// # use swait::*;
        /// # use way::*;
        /// # async {
        /// let (s, r) = bounded::<u64,8>();
        /// let receiver_clone=r.clone();
        /// assert_eq!(r.receiver_count().await,2);
        /// # }.swait();
        /// ```
        pub async fn receiver_count(&self) -> usize {
            self.internal.lock().await.recv_count
        }
        /// Returns count of alive sender instances of the channel.
        ///
        /// # Examples
        ///
        /// ```
        /// # use swait::*;
        /// # use way::*;
        /// # async {
        /// let (s, r) = bounded::<u64,8>();
        /// let sender_clone=s.clone();
        /// assert_eq!(r.sender_count().await,2);
        /// # }.swait();
        /// ```
        pub async fn sender_count(&self) -> usize {
            self.internal.lock().await.send_count
        }
        /// Closes the channel completely on both sides and terminates waiting
        /// signals.
        ///
        /// # Examples
        ///
        /// ```
        /// # use swait::*;
        /// # use way::*;
        /// # async {
        /// let (s, r) = bounded::<u64,8>();
        /// // closes channel on both sides and has same effect as r.close();
        /// s.close().await.unwrap();
        /// assert_eq!(r.is_closed().await,true);
        /// assert_eq!(s.is_closed().await,true);
        /// # }.swait();
        /// ```
        pub async fn close(&self) -> Result<(), CloseError> {
            let mut internal = self.internal.lock().await;
            if unlikely(internal.recv_count == 0 && internal.send_count == 0) {
                return Err(CloseError());
            }
            internal.recv_count = 0;
            internal.send_count = 0;
            internal.terminate_signals();
            internal.queue.clear();
            Ok(())
        }
        /// Returns whether the channel is closed on both side of send and
        /// receive or not.
        ///
        /// # Examples
        ///
        /// ```
        /// # use swait::*;
        /// # use way::*;
        /// # async {
        /// let (s, r) = bounded::<u64,8>();
        /// // closes channel on both sides and has same effect as r.close();
        /// s.close().await.unwrap();
        /// assert_eq!(r.is_closed().await,true);
        /// assert_eq!(s.is_closed().await,true);
        /// # }.swait();
        /// ```
        pub async fn is_closed(&self) -> bool {
            let internal = self.internal.lock().await;
            internal.send_count == 0 && internal.recv_count == 0
        }
    };
}

impl<T, const CAPACITY: usize> Sender<T, CAPACITY> {
    shared_impl!();
    #[cfg(feature = "sync")]
    pub fn as_sync(&self) -> &SyncSender<T, CAPACITY> {
        // SAFETY: SyncSender and Sender have the same layout
        unsafe { std::mem::transmute(self) }
    }

    #[cfg(feature = "sync")]
    pub fn to_sync(self) -> SyncSender<T, CAPACITY> {
        // SAFETY: SyncSender and Sender have the same layout
        unsafe { std::mem::transmute(self) }
    }
    pub async fn send(&self, data: T) -> Result<(), SendError<T>> {
        let mut guard = self.internal.lock().await;

        if unlikely(guard.recv_count == 0) {
            drop(guard);
            return Err(SendError(data));
        }
        if let Some(first) = guard.next_recv() {
            drop(guard);
            // SAFETY: it's safe to send to owned signal once
            unsafe { first.send::<T, CAPACITY>(data) }
            return Ok(());
        }
        if CAPACITY > 0 && guard.queue.len() < CAPACITY {
            // SAFETY: MaybeUninit is acting like a ManuallyDrop
            guard.queue.push_back(data);
            return Ok(());
        }

        let signal = pin!(SendSignal::new(&self.internal, data));
        guard.push_signal(signal.get_terminator());
        drop(guard);

        signal.await
    }

    pub async fn try_send(&self, data: T) -> Result<(), SendError<T>> {
        let mut guard = self.internal.lock().await;

        if unlikely(guard.recv_count == 0) {
            drop(guard);
            return Err(SendError(data));
        }
        if let Some(first) = guard.next_recv() {
            drop(guard);
            // SAFETY: it's safe to send to owned signal once
            unsafe { first.send::<T, CAPACITY>(data) }
            return Ok(());
        }
        if CAPACITY > 0 && guard.queue.len() < CAPACITY {
            guard.queue.push_back(data);
            return Ok(());
        }
        drop(guard);
        Err(SendError(data))
    }

    pub fn try_send_realtime(&self, data: T) -> Result<(), SendError<T>> {
        if let Some(mut guard) = self.internal.try_lock() {
            if unlikely(guard.recv_count == 0) {
                drop(guard);
                return Err(SendError(data));
            }
            if let Some(first) = guard.next_recv() {
                drop(guard);
                // SAFETY: it's safe to send to owned signal once
                unsafe { first.send::<T, CAPACITY>(data) }
                return Ok(());
            }
            if CAPACITY > 0 && guard.queue.len() < CAPACITY {
                guard.queue.push_back(data);
                return Ok(());
            }
        }
        Err(SendError(data))
    }

    pub async fn is_disconnected(&self) -> bool {
        let guard = self.internal.lock().await;
        guard.recv_count == 0
    }
}

impl<T, const CAPACITY: usize> Clone for Sender<T, CAPACITY> {
    fn clone(&self) -> Self {
        self.internal.lock_sync().inc_ref_count(true);
        Sender {
            internal: self.internal.clone(),
        }
    }
}

impl<T, const CAPACITY: usize> Drop for Sender<T, CAPACITY> {
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

impl<T, const CAPACITY: usize> Receiver<T, CAPACITY> {
    shared_impl!();
    #[cfg(feature = "sync")]
    pub fn as_sync(&self) -> &SyncReceiver<T, CAPACITY> {
        // SAFETY: SyncReceiver and Receiver have the same layout
        unsafe { std::mem::transmute(self) }
    }

    #[cfg(feature = "sync")]
    pub fn to_sync(self) -> SyncReceiver<T, CAPACITY> {
        // SAFETY: SyncReceiver and Receiver have the same layout
        unsafe { std::mem::transmute(self) }
    }
    pub async fn recv(&self) -> Result<T, ReceiveError> {
        let mut guard = self.internal.lock().await;

        if unlikely(guard.send_count == 0 && guard.queue.is_empty()) {
            drop(guard);
            return Err(ReceiveError());
        }
        if CAPACITY > 0 {
            if let Some(data) = guard.queue.pop_front() {
                if let Some(p) = guard.next_send() {
                    // SAFETY: it's safe to receive from owned signal once
                    unsafe { guard.queue.push_back(p.recv::<T, CAPACITY>()) }
                }
                return Ok(data);
            }
        }
        if let Some(first) = guard.next_send() {
            drop(guard);
            // SAFETY: it's safe to recv from owned signal once
            return Ok(unsafe { first.recv::<T, CAPACITY>() });
        }

        let signal = pin!(ReceiveSignal::new(&self.internal));
        guard.push_signal(signal.get_terminator());
        drop(guard);

        signal.await
    }
    pub async fn try_recv(&self) -> Result<T, ReceiveErrorTimeout> {
        let mut guard = self.internal.lock().await;

        if unlikely(guard.recv_count == 0) {
            return Err(ReceiveErrorTimeout::Closed);
        }
        if CAPACITY > 0 {
            if let Some(v) = guard.queue.pop_front() {
                if let Some(p) = guard.next_send() {
                    // SAFETY: it's safe to receive from owned signal once
                    unsafe { guard.queue.push_back(p.recv::<T, CAPACITY>()) }
                }
                return Ok(v);
            }
        }
        if let Some(p) = guard.next_send() {
            // SAFETY: it's safe to receive from owned signal once
            drop(guard);
            return unsafe { Ok(p.recv::<T, CAPACITY>()) };
        }
        if unlikely(guard.send_count == 0) {
            return Err(ReceiveErrorTimeout::Closed);
        }
        Err(ReceiveErrorTimeout::Timeout)
    }

    pub fn try_recv_realtime(&self) -> Result<T, ReceiveErrorTimeout> {
        if let Some(mut guard) = self.internal.try_lock() {
            if unlikely(guard.recv_count == 0) {
                return Err(ReceiveErrorTimeout::Closed);
            }
            if CAPACITY > 0 {
                if let Some(v) = guard.queue.pop_front() {
                    if let Some(p) = guard.next_send() {
                        // SAFETY: it's safe to receive from owned signal once
                        unsafe { guard.queue.push_back(p.recv::<T, CAPACITY>()) }
                    }
                    return Ok(v);
                }
            }
            if let Some(p) = guard.next_send() {
                // SAFETY: it's safe to receive from owned signal once
                drop(guard);
                return unsafe { Ok(p.recv::<T, CAPACITY>()) };
            }
            if unlikely(guard.send_count == 0) {
                return Err(ReceiveErrorTimeout::Closed);
            }
        }
        Err(ReceiveErrorTimeout::Timeout)
    }

    pub async fn drain_into(&self, vec: &mut Vec<T>) -> Result<usize, ReceiveError> {
        let vec_initial_length = vec.len();
        let remaining_cap = vec.capacity() - vec_initial_length;
        let mut guard = self.internal.lock().await;

        if unlikely(guard.recv_count == 0) {
            return Err(ReceiveError());
        }
        let required_cap = if CAPACITY > 0 { guard.queue.len() } else { 0 } + {
            if guard.recv_blocking {
                0
            } else {
                guard.wait_list.len()
            }
        };
        if required_cap > remaining_cap {
            vec.reserve(vec_initial_length + required_cap - remaining_cap);
        }
        if CAPACITY > 0 {
            while let Some(v) = guard.queue.pop_front() {
                vec.push(v);
            }
        }
        while let Some(p) = guard.next_send() {
            // SAFETY: it's safe to receive from owned signal once
            unsafe { vec.push(p.recv::<T, CAPACITY>()) }
        }
        Ok(required_cap)
    }

    pub async fn is_disconnected(&self) -> bool {
        let guard = self.internal.lock().await;
        guard.send_count == 0
    }

    pub async fn is_terminated(&self) -> bool {
        let guard = self.internal.lock().await;
        guard.send_count == 0 && guard.queue.is_empty()
    }
}

impl<T, const CAPACITY: usize> Clone for Receiver<T, CAPACITY> {
    fn clone(&self) -> Self {
        self.internal.lock_sync().inc_ref_count(false);
        Receiver {
            internal: self.internal.clone(),
        }
    }
}

impl<T, const CAPACITY: usize> Drop for Receiver<T, CAPACITY> {
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

pub fn zero<T>() -> (Sender<T>, Receiver<T>) {
    let internal = Internal::<T, 0>::new();
    (
        Sender {
            internal: internal.clone(),
        },
        Receiver { internal },
    )
}

pub fn bounded<T, const CAPACITY: usize>() -> (Sender<T, CAPACITY>, Receiver<T, CAPACITY>) {
    let internal = Internal::<T, CAPACITY>::new();
    (
        Sender {
            internal: internal.clone(),
        },
        Receiver { internal },
    )
}

#[cfg(test)]
#[cfg(feature = "std")]
mod tests {
    use core::time::Duration;

    use super::*;

    #[tokio::test]
    async fn test_basic_send_recv() {
        let (tx, rx) = bounded::<i32, 10>();
        tx.send(42).await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_multiple_sends() {
        let (tx, rx) = bounded::<i32, 10>();
        for i in 0..5 {
            tx.send(i).await.unwrap();
        }
        for i in 0..5 {
            assert_eq!(rx.recv().await.unwrap(), i);
        }
    }

    #[tokio::test]
    async fn test_send_after_disconnect() {
        let (tx, rx) = bounded::<i32, 10>();
        drop(rx);
        assert!(tx.send(42).await.is_err());
    }

    #[tokio::test]
    async fn test_recv_after_disconnect() {
        let (tx, rx) = bounded::<i32, 10>();
        drop(tx);
        assert!(rx.recv().await.is_err());
    }

    #[tokio::test]
    async fn test_clone_sender() {
        let (tx, rx) = bounded::<i32, 10>();
        let tx2 = tx.clone();
        tx.send(1).await.unwrap();
        tx2.send(2).await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), 1);
        assert_eq!(rx.recv().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_clone_receiver() {
        let (tx, rx) = bounded::<i32, 10>();
        let rx2 = rx.clone();
        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), 1);
        assert_eq!(rx2.recv().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_capacity_limit() {
        let (tx, rx) = bounded::<i32, 3>();
        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            rx.recv().await.unwrap();
        });

        tx.send(4).await.unwrap();
    }

    #[tokio::test]
    async fn test_try_send_success() {
        let (tx, rx) = bounded::<i32, 10>();
        assert!(tx.try_send(42).await.is_ok());
        assert_eq!(rx.recv().await.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_try_send_full() {
        let (tx, _rx) = bounded::<i32, 2>();
        tx.try_send(1).await.unwrap();
        tx.try_send(2).await.unwrap();
        assert!(tx.try_send(3).await.is_err());
    }

    #[tokio::test]
    async fn test_try_recv_empty() {
        let (_tx, rx) = bounded::<i32, 10>();
        assert_eq!(rx.try_recv().await, Err(ReceiveErrorTimeout::Timeout));
    }

    #[tokio::test]
    async fn test_try_recv_with_data() {
        let (tx, rx) = bounded::<i32, 10>();
        tx.send(42).await.unwrap();
        assert_eq!(rx.try_recv().await.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_try_send_realtime() {
        let (tx, rx) = bounded::<i32, 10>();
        assert!(tx.try_send_realtime(42).is_ok());
        assert_eq!(rx.recv().await.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_try_recv_realtime() {
        let (tx, rx) = bounded::<i32, 10>();
        tx.send(42).await.unwrap();
        assert_eq!(rx.try_recv_realtime().unwrap(), 42);
    }

    #[tokio::test]
    async fn test_drain_into() {
        let (tx, rx) = bounded::<i32, 10>();
        for i in 0..5 {
            tx.send(i).await.unwrap();
        }
        let mut vec = Vec::new();
        let count = rx.drain_into(&mut vec).await.unwrap();
        assert_eq!(count, 5);
        assert_eq!(vec, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn test_is_disconnected_sender() {
        let (tx, rx) = bounded::<i32, 10>();
        assert!(!tx.is_disconnected().await);
        drop(rx);
        assert!(tx.is_disconnected().await);
    }

    #[tokio::test]
    async fn test_is_disconnected_receiver() {
        let (tx, rx) = bounded::<i32, 10>();
        assert!(!rx.is_disconnected().await);
        drop(tx);
        assert!(rx.is_disconnected().await);
    }

    #[tokio::test]
    async fn test_is_terminated() {
        let (tx, rx) = bounded::<i32, 10>();
        tx.send(1).await.unwrap();
        assert!(!rx.is_terminated().await);
        drop(tx);
        assert!(!rx.is_terminated().await);
        rx.recv().await.unwrap();
        assert!(rx.is_terminated().await);
    }

    #[tokio::test]
    async fn test_zero_capacity() {
        let (tx, rx) = zero::<i32>();
        let tx_clone = tx.clone();
        let handle = tokio::spawn(async move {
            tx_clone.send(42).await.unwrap();
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(rx.recv().await.unwrap(), 42);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_concurrent_sends() {
        let (tx, rx) = bounded::<i32, 100>();
        let mut handles = vec![];
        for i in 0..10 {
            let tx_clone = tx.clone();
            handles.push(tokio::spawn(async move {
                tx_clone.send(i).await.unwrap();
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        drop(tx);
        let mut results = vec![];
        while let Ok(v) = rx.recv().await {
            results.push(v);
        }
        results.sort();
        assert_eq!(results, (0..10).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn test_concurrent_recvs() {
        let (tx, rx) = bounded::<i32, 100>();
        for i in 0..10 {
            tx.send(i).await.unwrap();
        }
        drop(tx);

        let mut handles = vec![];
        for _ in 0..10 {
            let rx_clone = rx.clone();
            handles.push(tokio::spawn(async move { rx_clone.recv().await.ok() }));
        }

        let mut results = vec![];
        for handle in handles {
            if let Some(v) = handle.await.unwrap() {
                results.push(v);
            }
        }
        results.sort();
        assert_eq!(results, (0..10).collect::<Vec<_>>());
    }
}
