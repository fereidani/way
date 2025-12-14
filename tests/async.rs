mod utils;
mod asyncs {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    //use futures_core::FusedStream;
    use way::{bounded, zero, ReceiveError, SendError};

    use crate::utils::*;

    macro_rules! mpmc_dyn {
        ($pre:stmt,$new:expr,$cap:expr) => {
            let (tx, rx) = bounded::<_,$cap>();
            let mut list = Vec::new();
            for _ in 0..THREADS {
                let tx = tx.clone();
                $pre
                let h = tokio::spawn(async move {
                    for _i in 0..MESSAGES / THREADS {
                        tx.send($new).await.unwrap();
                    }
                });
                list.push(h);
            }

            for _ in 0..THREADS {
                let rx = rx.clone();
                let h = tokio::spawn(async move {
                    for _i in 0..MESSAGES / THREADS {
                        rx.recv().await.unwrap();
                    }
                });
                list.push(h);
            }

            for h in list {
                h.await.unwrap();
            }
        };
    }

    macro_rules! integrity_test {
        ($zero:expr,$ones:expr) => {
            let (tx, rx) = zero::<_>();
            tokio::spawn(async move {
                for _ in 0..MESSAGES {
                    tx.send($zero).await.unwrap();
                    tx.send($ones).await.unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().await.unwrap(), $zero);
                assert_eq!(rx.recv().await.unwrap(), $ones);
            }
            let (tx, rx) = bounded::<_, 1>();
            tokio::spawn(async move {
                for _ in 0..MESSAGES {
                    tx.send($zero).await.unwrap();
                    tx.send($ones).await.unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().await.unwrap(), $zero);
                assert_eq!(rx.recv().await.unwrap(), $ones);
            }
            let (tx, rx) = zero::<_>();
            tokio::spawn(async move {
                for _ in 0..MESSAGES {
                    tx.send($zero).await.unwrap();
                    tx.send($ones).await.unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().await.unwrap(), $zero);
                assert_eq!(rx.recv().await.unwrap(), $ones);
            }
        };
    }

    async fn mpsc<const CAP: usize>() {
        let (tx, rx) = bounded::<Box<u8>, CAP>();
        let mut list = Vec::new();

        for _ in 0..THREADS {
            let tx = tx.clone();
            let h = tokio::spawn(async move {
                for _i in 0..MESSAGES / THREADS {
                    tx.send(Box::new(1)).await.unwrap();
                }
            });
            list.push(h);
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), Box::new(1));
        }

        for h in list {
            h.await.unwrap();
        }
    }

    async fn seq<const CAP: usize>() {
        let (tx, rx) = bounded::<Box<u8>, CAP>();

        for _i in 0..MESSAGES {
            tx.send(Box::new(1)).await.unwrap();
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), Box::new(1));
        }
    }

    async fn spsc<const CAP: usize>() {
        let (tx, rx) = bounded::<_, CAP>();

        tokio::spawn(async move {
            for _i in 0..MESSAGES {
                tx.send(Box::new(1)).await.unwrap();
            }
        });

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), Box::new(1));
        }
    }

    async fn mpmc<const CAP: usize>() {
        let (tx, rx) = bounded::<Box<u8>, CAP>();
        let mut list = Vec::new();
        for _ in 0..THREADS {
            let tx = tx.clone();
            let h = tokio::spawn(async move {
                for _i in 0..MESSAGES / THREADS {
                    tx.send(Box::new(1)).await.unwrap();
                }
            });
            list.push(h);
        }

        for _ in 0..THREADS {
            let rx = rx.clone();
            let h = tokio::spawn(async move {
                for _i in 0..MESSAGES / THREADS {
                    rx.recv().await.unwrap();
                }
            });
            list.push(h);
        }

        for h in list {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn integrity_u8() {
        integrity_test!(0u8, !0u8);
    }

    #[tokio::test]
    async fn integrity_u16() {
        integrity_test!(0u16, !0u16);
    }

    #[tokio::test]
    async fn integrity_u32() {
        integrity_test!(0u32, !0u32);
    }

    #[tokio::test]
    async fn integrity_usize() {
        integrity_test!(0u64, !0u64);
    }

    #[tokio::test]
    async fn integrity_big() {
        integrity_test!((0u64, 0u64, 0u64, 0u64), (!0u64, !0u64, !0u64, !0u64));
    }

    #[tokio::test]
    async fn integrity_string() {
        integrity_test!("", "not empty");
    }

    #[tokio::test]
    async fn integrity_padded_rust() {
        integrity_test!(
            Padded {
                a: false,
                b: 0x0,
                c: 0x0
            },
            Padded {
                a: true,
                b: 0xFF,
                c: 0xFFFFFFFF
            }
        );
    }

    #[tokio::test]
    async fn integrity_padded_c() {
        integrity_test!(
            PaddedReprC {
                a: false,
                b: 0x0,
                c: 0x0
            },
            PaddedReprC {
                a: true,
                b: 0xFF,
                c: 0xFFFFFFFF
            }
        );
    }

    #[tokio::test]
    async fn drop_test() {
        let counter = Arc::new(AtomicUsize::new(0));
        mpmc_dyn!(let counter=counter.clone(),DropTester::new(counter.clone(), 10), 1);
        assert_eq!(counter.load(Ordering::SeqCst), MESSAGES);
    }

    #[tokio::test]
    async fn drop_test_in_signal() {
        let (s, r) = bounded::<_, 10>();

        let counter = Arc::new(AtomicUsize::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let counter = counter.clone();
            let s = s.clone();
            let c = tokio::spawn(async move {
                let _ = s.send(DropTester::new(counter, 1234)).await;
            });
            list.push(c);
        }
        r.close().await.unwrap();
        for c in list {
            c.await.unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    // Channel logic tests
    #[tokio::test]
    async fn recv_from_half_closed_queue() {
        let (tx, rx) = bounded::<_, 1>();
        tx.send(Box::new(1)).await.unwrap();
        drop(tx);
        // it's ok to receive data from queue of half closed channel
        assert_eq!(rx.recv().await.unwrap(), Box::new(1));
    }

    #[tokio::test]
    async fn recv_from_half_closed_channel() {
        let (tx, rx) = bounded::<u8, 1>();
        drop(tx);
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError());
    }

    #[tokio::test]
    async fn recv_from_closed_channel() {
        let (tx, rx) = bounded::<u8, 1>();
        tx.close().await.unwrap();
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError());
    }

    #[tokio::test]
    async fn recv_from_closed_channel_queue() {
        let (tx, rx) = bounded::<_, 1>();
        tx.send(Box::new(1)).await.unwrap();
        tx.close().await.unwrap();
        // it's not possible to read data from queue of fully closed channel
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError());
    }

    #[tokio::test]
    async fn send_to_half_closed_channel() {
        let (tx, rx) = bounded::<_, 1>();
        drop(rx);
        assert!(matches!(
            tx.send(Box::new(1)).await.err().unwrap(),
            SendError(_)
        ));
    }

    #[tokio::test]
    async fn send_to_closed_channel() {
        let (tx, rx) = bounded::<_, 1>();
        rx.close().await.unwrap();
        assert!(matches!(
            tx.send(Box::new(1)).await.err().unwrap(),
            SendError(_)
        ));
    }

    // Drop tests
    #[tokio::test]
    async fn recv_abort_test() {
        let (_s, r) = bounded::<u64, 10>();

        let mut list = Vec::new();
        for _ in 0..10 {
            let r = r.clone();
            let c = tokio::spawn(async move {
                if r.recv().await.is_ok() {
                    panic!("should not be ok");
                }
            });
            list.push(c);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        for c in list {
            c.abort();
        }
        r.close().await.unwrap();
    }

    // Drop tests
    #[tokio::test]
    async fn send_abort_test() {
        let (s, r) = zero();
        let counter = Arc::new(AtomicUsize::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let s = s.clone();
            let counter = counter.clone();
            let c = tokio::spawn(async move {
                if s.send(DropTester::new(counter, 1234)).await.is_ok() {
                    panic!("should not be ok");
                }
            });
            list.push(c);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        for c in list {
            c.abort();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
        r.close().await.unwrap();
    }

    #[tokio::test]
    async fn drop_test_in_queue() {
        let (s, r) = bounded::<DropTester, 10>();

        let counter = Arc::new(AtomicUsize::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let counter = counter.clone();
            let s = s.clone();
            let c = tokio::spawn(async move {
                let _ = s.send(DropTester::new(counter, 1234)).await;
            });
            list.push(c);
        }
        for c in list {
            c.await.unwrap();
        }
        r.close().await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[tokio::test]
    async fn drop_test_in_unused_signal() {
        let (s, r) = bounded::<_, 10>();

        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            drop(s.send(DropTester::new(counter, 1234)));
        }
        r.close().await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[tokio::test]
    async fn drop_test_send_to_closed() {
        let (s, r) = bounded::<_, 10>();
        r.close().await.unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234)).await;
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[tokio::test]
    async fn drop_test_send_to_half_closed() {
        let (s, r) = bounded::<_, 10>();
        drop(r);
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234)).await;
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[tokio::test]
    async fn vec_test() {
        mpmc_dyn!({}, vec![1, 2, 3], 1);
    }

    #[tokio::test]
    async fn one_msg() {
        let (s, r) = bounded::<u8, 1>();
        s.send(0).await.unwrap();
        assert_eq!(r.recv().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn mpsc_0() {
        mpsc::<0>().await;
    }

    #[tokio::test]
    async fn mpsc_n() {
        mpsc::<MESSAGES>().await;
    }

    #[tokio::test]
    async fn mpsc_u() {
        mpsc::<0>().await;
    }

    #[tokio::test]
    async fn mpmc_0() {
        mpmc::<0>().await;
    }

    #[tokio::test]
    async fn mpmc_n() {
        mpmc::<MESSAGES>().await;
    }

    #[tokio::test]
    async fn spsc_0() {
        spsc::<0>().await;
    }

    #[tokio::test]
    async fn spsc_1() {
        spsc::<1>().await;
    }

    #[tokio::test]
    async fn spsc_n() {
        spsc::<MESSAGES>().await;
    }

    #[tokio::test]
    async fn seq_n() {
        seq::<MESSAGES>().await;
    }

    /*
    #[tokio::test]
    async fn stream() {
        use futures::stream::StreamExt;
        let (s, r) = new::<0>();
        tokio::spawn(async move {
            for i in 0..MESSAGES {
                s.send(i).await.unwrap();
            }
        });
        let mut stream = r.stream();

        assert!(!stream.is_terminated());
        for i in 0..MESSAGES {
            assert_eq!(stream.next().await.unwrap(), i);
        }
        assert_eq!(stream.next().await, None);
        assert!(stream.is_terminated());
        assert_eq!(stream.next().await, None);
    }
     */

    #[tokio::test]
    async fn spsc_overaligned_zst() {
        #[repr(align(1024))]
        struct Foo;

        let (tx, rx) = bounded::<Foo, 0>();

        tokio::spawn(async move {
            for _i in 0..MESSAGES {
                tx.send(Foo).await.unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().await.unwrap();
        }
    }
}
