use core::time::Duration;

#[cfg(feature = "std")]
pub(crate) fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

#[cfg(all(not(feature = "std"), unix))]
pub(crate) fn sleep(duration: Duration) {
    let micros = duration.as_micros().min(u32::MAX as u128) as u32;
    unsafe {
        libc::usleep(micros);
    }
}

#[cfg(all(not(feature = "std"), windows))]
pub(crate) fn sleep(duration: Duration) {
    let millis = duration.as_millis().min(u32::MAX as u128) as u32;
    unsafe {
        winapi::um::synchapi::Sleep(millis);
    }
}

//TODO: improve for no_std no_os targets
#[cfg(all(not(feature = "std"), target_os = "none"))]
pub(crate) fn sleep(duration: Duration) {
    let nanos = duration.as_nanos();
    for _ in 0..nanos {
        core::hint::spin_loop();
    }
}

#[cfg(all(not(feature = "std"), target_os = "none"))]
pub(crate) fn yield_now() {
    sleep(Duration::from_nanos(128));
}

#[cfg(feature = "std")]
pub(crate) fn yield_now() {
    std::thread::yield_now();
}

#[cfg(all(not(feature = "std"), unix))]
pub(crate) fn yield_now() {
    unsafe {
        libc::sched_yield();
    }
}

#[cfg(all(not(feature = "std"), windows))]
pub(crate) fn yield_now() {
    unsafe {
        winapi::um::processthreadsapi::SwitchToThread();
    }
}
