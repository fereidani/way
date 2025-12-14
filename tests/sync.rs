mod utils;
#[cfg(feature = "sync")]
mod syncs {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        thread,
    };

    use way::{bounded_sync, zero_sync, ReceiveError, SendError};

    use crate::utils::*;

    macro_rules! mpmc_dyn {
        ($pre:stmt,$new:expr,$cap:expr) => {
            let (tx, rx) = bounded_sync::<_,$cap>();
            let mut list = Vec::new();
            for _ in 0..THREADS {
                let tx = tx.clone();
                $pre
                let h = thread::spawn(move || {
                    for _i in 0..MESSAGES / THREADS {
                        tx.send($new).unwrap();
                    }
                });
                list.push(h);
            }

            for _ in 0..THREADS {
                let rx = rx.clone();
                let h = thread::spawn(move || {
                    for _i in 0..MESSAGES / THREADS {
                        rx.recv().unwrap();
                    }
                });
                list.push(h);
            }

            for h in list {
                h.join().unwrap();
            }
        };
    }

    macro_rules! integrity_test {
        ($zero:expr,$ones:expr) => {
            let (tx, rx) = zero_sync::<_>();
            thread::spawn(move || {
                for _ in 0..MESSAGES {
                    tx.send($zero).unwrap();
                    tx.send($ones).unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().unwrap(), $zero);
                assert_eq!(rx.recv().unwrap(), $ones);
            }
            let (tx, rx) = bounded_sync::<_, 1>();
            thread::spawn(move || {
                for _ in 0..MESSAGES {
                    tx.send($zero).unwrap();
                    tx.send($ones).unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().unwrap(), $zero);
                assert_eq!(rx.recv().unwrap(), $ones);
            }
            let (tx, rx) = zero_sync::<_>();
            thread::spawn(move || {
                for _ in 0..MESSAGES {
                    tx.send($zero).unwrap();
                    tx.send($ones).unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().unwrap(), $zero);
                assert_eq!(rx.recv().unwrap(), $ones);
            }
        };
    }

    fn mpsc<const CAP: usize>() {
        let (tx, rx) = bounded_sync::<Box<u8>, CAP>();
        let mut list = Vec::new();

        for _ in 0..THREADS {
            let tx = tx.clone();
            let h = thread::spawn(move || {
                for _i in 0..MESSAGES / THREADS {
                    tx.send(Box::new(1)).unwrap();
                }
            });
            list.push(h);
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().unwrap(), Box::new(1));
        }

        for h in list {
            h.join().unwrap();
        }
    }

    fn seq<const CAP: usize>() {
        let (tx, rx) = bounded_sync::<Box<u8>, CAP>();

        for _i in 0..MESSAGES {
            tx.send(Box::new(1)).unwrap();
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().unwrap(), Box::new(1));
        }
    }

    fn spsc<const CAP: usize>() {
        let (tx, rx) = bounded_sync::<_, CAP>();

        thread::spawn(move || {
            for _i in 0..MESSAGES {
                tx.send(Box::new(1)).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().unwrap(), Box::new(1));
        }
    }

    fn mpmc<const CAP: usize>() {
        let (tx, rx) = bounded_sync::<Box<u8>, CAP>();
        let mut list = Vec::new();
        for _ in 0..THREADS {
            let tx = tx.clone();
            let h = thread::spawn(move || {
                for _i in 0..MESSAGES / THREADS {
                    tx.send(Box::new(1)).unwrap();
                }
            });
            list.push(h);
        }

        for _ in 0..THREADS {
            let rx = rx.clone();
            let h = thread::spawn(move || {
                for _i in 0..MESSAGES / THREADS {
                    rx.recv().unwrap();
                }
            });
            list.push(h);
        }

        for h in list {
            h.join().unwrap();
        }
    }

    #[test]
    fn integrity_u8() {
        integrity_test!(0u8, !0u8);
    }

    #[test]
    fn integrity_u16() {
        integrity_test!(0u16, !0u16);
    }

    #[test]
    fn integrity_u32() {
        integrity_test!(0u32, !0u32);
    }

    #[test]
    fn integrity_usize() {
        integrity_test!(0u64, !0u64);
    }

    #[test]
    fn integrity_big() {
        integrity_test!((0u64, 0u64, 0u64, 0u64), (!0u64, !0u64, !0u64, !0u64));
    }

    #[test]
    fn integrity_string() {
        integrity_test!("", "not empty");
    }

    #[test]
    fn integrity_padded_rust() {
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

    #[test]
    fn integrity_padded_c() {
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

    #[test]
    fn drop_test() {
        let counter = Arc::new(AtomicUsize::new(0));
        mpmc_dyn!(let counter=counter.clone(),DropTester::new(counter.clone(), 10), 1);
        assert_eq!(counter.load(Ordering::SeqCst), MESSAGES);
    }

    #[test]
    fn drop_test_in_signal() {
        let (s, r) = bounded_sync::<_, 10>();

        let counter = Arc::new(AtomicUsize::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let counter = counter.clone();
            let s = s.clone();
            let c = thread::spawn(move || {
                let _ = s.send(DropTester::new(counter, 1234));
            });
            list.push(c);
        }
        r.close().unwrap();
        for c in list {
            c.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    // Channel logic tests
    #[test]
    fn recv_from_half_closed_queue() {
        let (tx, rx) = bounded_sync::<_, 1>();
        tx.send(Box::new(1)).unwrap();
        drop(tx);
        // it's ok to receive data from queue of half closed channel
        assert_eq!(rx.recv().unwrap(), Box::new(1));
    }

    #[test]
    fn recv_from_half_closed_channel() {
        let (tx, rx) = bounded_sync::<u8, 1>();
        drop(tx);
        assert_eq!(rx.recv().err().unwrap(), ReceiveError());
    }

    #[test]
    fn recv_from_closed_channel() {
        let (tx, rx) = bounded_sync::<u8, 1>();
        tx.close().unwrap();
        assert_eq!(rx.recv().err().unwrap(), ReceiveError());
    }

    #[test]
    fn recv_from_closed_channel_queue() {
        let (tx, rx) = bounded_sync::<_, 1>();
        tx.send(Box::new(1)).unwrap();
        tx.close().unwrap();
        // it's not possible to read data from queue of fully closed channel
        assert_eq!(rx.recv().err().unwrap(), ReceiveError());
    }

    #[test]
    fn send_to_half_closed_channel() {
        let (tx, rx) = bounded_sync::<_, 1>();
        drop(rx);
        assert!(matches!(tx.send(Box::new(1)).err().unwrap(), SendError(_)));
    }

    #[test]
    fn send_to_closed_channel() {
        let (tx, rx) = bounded_sync::<_, 1>();
        rx.close().unwrap();
        assert!(matches!(tx.send(Box::new(1)).err().unwrap(), SendError(_)));
    }

    #[test]
    fn drop_test_in_queue() {
        let (s, r) = bounded_sync::<DropTester, 10>();

        let counter = Arc::new(AtomicUsize::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let counter = counter.clone();
            let s = s.clone();
            let c = thread::spawn(move || {
                let _ = s.send(DropTester::new(counter, 1234));
            });
            list.push(c);
        }
        for c in list {
            c.join().unwrap();
        }
        r.close().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[test]
    fn drop_test_in_unused_signal() {
        let (s, r) = bounded_sync::<_, 10>();

        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            drop(s.send(DropTester::new(counter, 1234)));
        }
        r.close().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[test]
    fn drop_test_send_to_closed() {
        let (s, r) = bounded_sync::<_, 10>();
        r.close().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[test]
    fn drop_test_send_to_half_closed() {
        let (s, r) = bounded_sync::<_, 10>();
        drop(r);
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[test]
    fn vec_test() {
        mpmc_dyn!({}, vec![1, 2, 3], 1);
    }

    #[test]
    fn one_msg() {
        let (s, r) = bounded_sync::<u8, 1>();
        s.send(0).unwrap();
        assert_eq!(r.recv().unwrap(), 0);
    }

    #[test]
    fn mpsc_0() {
        mpsc::<0>();
    }

    #[test]
    fn mpsc_n() {
        mpsc::<MESSAGES>();
    }

    #[test]
    fn mpsc_u() {
        mpsc::<0>();
    }

    #[test]
    fn mpmc_0() {
        mpmc::<0>();
    }

    #[test]
    fn mpmc_n() {
        mpmc::<MESSAGES>();
    }

    #[test]
    fn spsc_0() {
        spsc::<0>();
    }

    #[test]
    fn spsc_1() {
        spsc::<1>();
    }

    #[test]
    fn spsc_n() {
        spsc::<MESSAGES>();
    }

    #[test]
    fn seq_n() {
        seq::<MESSAGES>();
    }

    #[test]
    fn spsc_overaligned_zst() {
        #[repr(align(1024))]
        struct Foo;

        let (tx, rx) = bounded_sync::<Foo, 0>();

        thread::spawn(move || {
            for _i in 0..MESSAGES {
                tx.send(Foo).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    }
}
