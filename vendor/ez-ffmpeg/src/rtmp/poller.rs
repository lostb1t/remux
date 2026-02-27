// src/rtmp/poller.rs - Cross-platform IO multiplexer
//
// Provides a unified IO multiplexing abstraction:
// - Linux: epoll (edge-triggered)
// - macOS/BSD: kqueue (EV_CLEAR edge-triggered)
// - Windows: WSAPoll (level-triggered)
//
// Design principles:
// - No new dependencies, uses std + libc FFI
// - Edge-triggered mode requires drain until WouldBlock
// - EINTR auto-retry

use std::io;
use std::time::Duration;

/// Event interest flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interest {
    pub readable: bool,
    pub writable: bool,
}

impl Interest {
    pub const READABLE: Interest = Interest {
        readable: true,
        writable: false,
    };

    #[cfg(test)]
    pub const WRITABLE: Interest = Interest {
        readable: false,
        writable: true,
    };

    pub fn add_writable(self) -> Interest {
        Interest {
            writable: true,
            ..self
        }
    }
}

/// IO event
#[derive(Debug, Clone, Copy)]
pub struct Event {
    pub token: usize,
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
    pub hangup: bool,
}

impl Event {
    pub fn is_readable(&self) -> bool {
        self.readable
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    pub fn is_error(&self) -> bool {
        self.error
    }

    pub fn is_hangup(&self) -> bool {
        self.hangup
    }
}

// ============================================================================
// Platform-specific implementations
// ============================================================================

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::os::unix::io::RawFd;

    pub type RawHandle = RawFd;

    // epoll constants
    const EPOLL_CTL_ADD: i32 = 1;
    const EPOLL_CTL_DEL: i32 = 2;
    const EPOLL_CTL_MOD: i32 = 3;

    const EPOLLIN: u32 = 0x001;
    const EPOLLOUT: u32 = 0x004;
    const EPOLLERR: u32 = 0x008;
    const EPOLLHUP: u32 = 0x010;
    const EPOLLET: u32 = 1 << 31; // Edge-triggered

    #[repr(C)]
    #[derive(Clone, Copy)]
    union EpollData {
        ptr: *mut std::ffi::c_void,
        fd: i32,
        u32: u32,
        u64: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct EpollEvent {
        events: u32,
        data: EpollData,
    }

    // # Safety
    //
    // These FFI functions directly call Linux epoll system calls.
    // Callers must ensure:
    // - `epfd` is a valid epoll file descriptor created by `epoll_create1`
    // - `fd` is a valid file descriptor
    // - `event` points to a valid `EpollEvent` or is null (for `EPOLL_CTL_DEL`)
    // - `events` points to a valid array with at least `maxevents` capacity
    // - File descriptors are not closed while registered with epoll
    extern "C" {
        fn epoll_create1(flags: i32) -> i32;
        fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: *mut EpollEvent) -> i32;
        fn epoll_wait(epfd: i32, events: *mut EpollEvent, maxevents: i32, timeout: i32) -> i32;
        fn close(fd: i32) -> i32;
    }

    pub struct Poller {
        epfd: RawFd,
    }

    impl Poller {
        pub fn new() -> io::Result<Self> {
            // SAFETY: epoll_create1(0) is a safe syscall that:
            // - Takes no pointers or external resources
            // - Returns a new file descriptor or -1 on error
            // - Error is checked immediately after the call
            // Thread safety: Creating an epoll instance is thread-safe
            let epfd = unsafe { epoll_create1(0) };
            if epfd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Poller { epfd })
        }

        pub fn register(&mut self, fd: RawHandle, token: usize, interest: Interest) -> io::Result<()> {
            let events = interest_to_epoll(interest) | EPOLLET;
            let mut event = EpollEvent {
                events,
                data: EpollData { u64: token as u64 },
            };

            // SAFETY: epoll_ctl with EPOLL_CTL_ADD requires:
            // - self.epfd is valid (created in new(), owned by self)
            // - fd is a valid file descriptor (caller's responsibility per API contract)
            // - &mut event points to a valid, properly initialized EpollEvent on the stack
            // Error is checked immediately; operation is atomic w.r.t. this epoll instance
            // Thread safety: Poller requires &mut self, ensuring exclusive access
            let ret = unsafe { epoll_ctl(self.epfd, EPOLL_CTL_ADD, fd, &mut event) };
            if ret < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn modify(&mut self, fd: RawHandle, token: usize, interest: Interest) -> io::Result<()> {
            let events = interest_to_epoll(interest) | EPOLLET;
            let mut event = EpollEvent {
                events,
                data: EpollData { u64: token as u64 },
            };

            // SAFETY: epoll_ctl with EPOLL_CTL_MOD requires:
            // - self.epfd is valid (created in new(), owned by self)
            // - fd was previously registered (caller's responsibility per API contract)
            // - &mut event points to a valid, properly initialized EpollEvent on the stack
            // Error is checked immediately; operation is atomic w.r.t. this epoll instance
            // Thread safety: Poller requires &mut self, ensuring exclusive access
            let ret = unsafe { epoll_ctl(self.epfd, EPOLL_CTL_MOD, fd, &mut event) };
            if ret < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn deregister(&mut self, fd: RawHandle) -> io::Result<()> {
            // SAFETY: epoll_ctl with EPOLL_CTL_DEL requires:
            // - self.epfd is valid (created in new(), owned by self)
            // - fd was previously registered (caller's responsibility per API contract)
            // - event pointer can be null for EPOLL_CTL_DEL (per Linux kernel 2.6.9+)
            // Error is checked immediately; operation is atomic w.r.t. this epoll instance
            // Thread safety: Poller requires &mut self, ensuring exclusive access
            let ret = unsafe { epoll_ctl(self.epfd, EPOLL_CTL_DEL, fd, std::ptr::null_mut()) };
            if ret < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<Vec<Event>> {
            let timeout_ms = timeout
                .map(|d| d.as_millis() as i32)
                .unwrap_or(-1);

            // SAFETY: std::mem::zeroed() for EpollEvent array is safe because:
            // - EpollEvent is a POD type with no invalid bit patterns
            // - All zero bytes represent valid (empty) events
            // - The array is immediately overwritten by epoll_wait
            let mut events: [EpollEvent; 256] = unsafe { std::mem::zeroed() };

            loop {
                // SAFETY: epoll_wait requires:
                // - self.epfd is valid (created in new(), owned by self)
                // - events.as_mut_ptr() points to valid, writable memory for 256 EpollEvents
                // - events.len() correctly reports the array capacity
                // - timeout_ms is a valid i32 (-1 for infinite, >=0 for milliseconds)
                // Error (including EINTR) is checked immediately
                // Thread safety: Poller requires &mut self, ensuring exclusive access
                let ret = unsafe {
                    epoll_wait(self.epfd, events.as_mut_ptr(), events.len() as i32, timeout_ms)
                };

                if ret < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue; // EINTR - retry
                    }
                    return Err(err);
                }

                let mut result = Vec::with_capacity(ret as usize);
                for i in 0..ret as usize {
                    let ev = &events[i];
                    result.push(Event {
                        // SAFETY: We always use the u64 field for token storage.
                        // register() writes u64, poll() reads u64 - same field, same layout.
                        // This is the standard pattern for epoll_data_t union in Rust FFI.
                        token: unsafe { ev.data.u64 } as usize,
                        readable: (ev.events & EPOLLIN) != 0,
                        writable: (ev.events & EPOLLOUT) != 0,
                        error: (ev.events & EPOLLERR) != 0,
                        hangup: (ev.events & EPOLLHUP) != 0,
                    });
                }
                return Ok(result);
            }
        }
    }

    impl Drop for Poller {
        fn drop(&mut self) {
            // SAFETY: close() on self.epfd is safe because:
            // - self.epfd is valid (created in new(), owned exclusively by self)
            // - This is the only place where epfd is closed (Drop is called once)
            // - After drop, self is deallocated so no double-close is possible
            // Thread safety: Drop takes &mut self, ensuring exclusive access
            unsafe { close(self.epfd) };
        }
    }

    fn interest_to_epoll(interest: Interest) -> u32 {
        let mut events = 0;
        if interest.readable {
            events |= EPOLLIN;
        }
        if interest.writable {
            events |= EPOLLOUT;
        }
        events
    }
}

#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
mod bsd {
    use super::*;
    use std::os::unix::io::RawFd;

    pub type RawHandle = RawFd;

    // kqueue constants
    const EVFILT_READ: i16 = -1;
    const EVFILT_WRITE: i16 = -2;

    const EV_ADD: u16 = 0x0001;
    const EV_DELETE: u16 = 0x0002;
    const EV_ENABLE: u16 = 0x0004;
    const EV_CLEAR: u16 = 0x0020; // Edge-triggered equivalent
    const EV_EOF: u16 = 0x8000;
    const EV_ERROR: u16 = 0x4000;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Timespec {
        tv_sec: isize,
        tv_nsec: isize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Kevent {
        ident: usize,
        filter: i16,
        flags: u16,
        fflags: u32,
        data: isize,
        udata: *mut std::ffi::c_void,
    }

    // # Safety
    //
    // These FFI functions directly call BSD kqueue system calls.
    // Callers must ensure:
    // - `kq` is a valid kqueue descriptor created by `kqueue()`
    // - `changelist` points to a valid array of `Kevent` with at least `nchanges` elements
    // - `eventlist` points to a valid array with at least `nevents` capacity
    // - `timeout` points to a valid `Timespec` or is null for blocking
    // - File descriptors referenced in kevents are valid and not closed while registered
    extern "C" {
        fn kqueue() -> i32;
        fn kevent(
            kq: i32,
            changelist: *const Kevent,
            nchanges: i32,
            eventlist: *mut Kevent,
            nevents: i32,
            timeout: *const Timespec,
        ) -> i32;
        fn close(fd: i32) -> i32;
    }

    pub struct Poller {
        kq: RawFd,
    }

    impl Poller {
        pub fn new() -> io::Result<Self> {
            // SAFETY: kqueue() is a safe syscall that:
            // - Takes no pointers or external resources
            // - Returns a new file descriptor or -1 on error
            // - Error is checked immediately after the call
            // Thread safety: Creating a kqueue instance is thread-safe
            let kq = unsafe { kqueue() };
            if kq < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Poller { kq })
        }

        pub fn register(&mut self, fd: RawHandle, token: usize, interest: Interest) -> io::Result<()> {
            let mut changes = Vec::with_capacity(2);

            if interest.readable {
                changes.push(Kevent {
                    ident: fd as usize,
                    filter: EVFILT_READ,
                    flags: EV_ADD | EV_ENABLE | EV_CLEAR,
                    fflags: 0,
                    data: 0,
                    udata: token as *mut _,
                });
            }

            if interest.writable {
                changes.push(Kevent {
                    ident: fd as usize,
                    filter: EVFILT_WRITE,
                    flags: EV_ADD | EV_ENABLE | EV_CLEAR,
                    fflags: 0,
                    data: 0,
                    udata: token as *mut _,
                });
            }

            if changes.is_empty() {
                return Ok(());
            }

            // SAFETY: kevent() for registration requires:
            // - self.kq is valid (created in new(), owned by self)
            // - changes.as_ptr() points to valid Kevent array with correct length
            // - eventlist is null (we're only submitting changes, not polling)
            // - timeout is null (no wait needed for change submission)
            // Error is checked immediately
            // Thread safety: Poller requires &mut self, ensuring exclusive access
            let ret = unsafe {
                kevent(
                    self.kq,
                    changes.as_ptr(),
                    changes.len() as i32,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };

            if ret < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn modify(&mut self, fd: RawHandle, token: usize, interest: Interest) -> io::Result<()> {
            // kqueue: For modify, we use EV_ADD which will update existing registration
            // Note: We need to explicitly disable filters we don't want anymore
            let mut changes = Vec::with_capacity(2);

            // For EVFILT_READ
            if interest.readable {
                changes.push(Kevent {
                    ident: fd as usize,
                    filter: EVFILT_READ,
                    flags: EV_ADD | EV_ENABLE | EV_CLEAR,
                    fflags: 0,
                    data: 0,
                    udata: token as *mut _,
                });
            } else {
                // Disable read filter
                changes.push(Kevent {
                    ident: fd as usize,
                    filter: EVFILT_READ,
                    flags: EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: std::ptr::null_mut(),
                });
            }

            // For EVFILT_WRITE
            if interest.writable {
                changes.push(Kevent {
                    ident: fd as usize,
                    filter: EVFILT_WRITE,
                    flags: EV_ADD | EV_ENABLE | EV_CLEAR,
                    fflags: 0,
                    data: 0,
                    udata: token as *mut _,
                });
            } else {
                // Disable write filter
                changes.push(Kevent {
                    ident: fd as usize,
                    filter: EVFILT_WRITE,
                    flags: EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: std::ptr::null_mut(),
                });
            }

            // Apply changes - ignore errors from EV_DELETE on non-existent filters
            // SAFETY: kevent() for modification requires:
            // - self.kq is valid (created in new(), owned by self)
            // - changes.as_ptr() points to valid Kevent array with correct length
            // - eventlist is null (we're only submitting changes, not polling)
            // - timeout is null (no wait needed for change submission)
            // EV_DELETE on non-existent filter may return error (expected behavior)
            // Thread safety: Poller requires &mut self, ensuring exclusive access
            let ret = unsafe {
                kevent(
                    self.kq,
                    changes.as_ptr(),
                    changes.len() as i32,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };

            // Only fail if we were adding and it failed
            if ret < 0 && (interest.readable || interest.writable) {
                // Log warning but don't fail - kqueue EV_DELETE on non-existent filter is expected
                log::warn!(
                    "kqueue modify returned error for fd {}: {}",
                    fd,
                    std::io::Error::last_os_error()
                );
            }
            Ok(())
        }

        pub fn deregister(&mut self, fd: RawHandle) -> io::Result<()> {
            let changes = [
                Kevent {
                    ident: fd as usize,
                    filter: EVFILT_READ,
                    flags: EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: std::ptr::null_mut(),
                },
                Kevent {
                    ident: fd as usize,
                    filter: EVFILT_WRITE,
                    flags: EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: std::ptr::null_mut(),
                },
            ];

            // Ignore errors - filter might not be registered
            // SAFETY: kevent() for deregistration requires:
            // - self.kq is valid (created in new(), owned by self)
            // - changes.as_ptr() points to valid Kevent array with correct length
            // - eventlist is null (we're only submitting changes, not polling)
            // - timeout is null (no wait needed for change submission)
            // EV_DELETE errors are intentionally ignored (filter might not exist)
            // Thread safety: Poller requires &mut self, ensuring exclusive access
            unsafe {
                kevent(
                    self.kq,
                    changes.as_ptr(),
                    changes.len() as i32,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                );
            }
            Ok(())
        }

        pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<Vec<Event>> {
            let timespec = timeout.map(|d| Timespec {
                tv_sec: d.as_secs() as isize,
                tv_nsec: d.subsec_nanos() as isize,
            });

            let timeout_ptr = timespec
                .as_ref()
                .map(|t| t as *const _)
                .unwrap_or(std::ptr::null());

            // SAFETY: std::mem::zeroed() for Kevent array is safe because:
            // - Kevent is a POD type with no invalid bit patterns
            // - All zero bytes represent valid (empty) events
            // - The array is immediately overwritten by kevent()
            let mut events: [Kevent; 256] = unsafe { std::mem::zeroed() };

            loop {
                // SAFETY: kevent() for polling requires:
                // - self.kq is valid (created in new(), owned by self)
                // - changelist is null (no changes to submit)
                // - events.as_mut_ptr() points to valid, writable memory for 256 Kevents
                // - events.len() correctly reports the array capacity
                // - timeout_ptr points to valid Timespec or is null for blocking
                // Error (including EINTR) is checked immediately
                // Thread safety: Poller requires &mut self, ensuring exclusive access
                let ret = unsafe {
                    kevent(
                        self.kq,
                        std::ptr::null(),
                        0,
                        events.as_mut_ptr(),
                        events.len() as i32,
                        timeout_ptr,
                    )
                };

                if ret < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue; // EINTR - retry
                    }
                    return Err(err);
                }

                // Aggregate events by token
                use std::collections::HashMap;
                let mut event_map: HashMap<usize, Event> = HashMap::new();

                for i in 0..ret as usize {
                    let ev = &events[i];
                    let token = ev.udata as usize;

                    let entry = event_map.entry(token).or_insert(Event {
                        token,
                        readable: false,
                        writable: false,
                        error: (ev.flags & EV_ERROR) != 0,
                        hangup: (ev.flags & EV_EOF) != 0,
                    });

                    match ev.filter {
                        EVFILT_READ => entry.readable = true,
                        EVFILT_WRITE => entry.writable = true,
                        _ => {}
                    }

                    if (ev.flags & EV_ERROR) != 0 {
                        entry.error = true;
                    }
                    if (ev.flags & EV_EOF) != 0 {
                        entry.hangup = true;
                    }
                }

                return Ok(event_map.into_values().collect());
            }
        }
    }

    impl Drop for Poller {
        fn drop(&mut self) {
            // SAFETY: close() on self.kq is safe because:
            // - self.kq is valid (created in new(), owned exclusively by self)
            // - This is the only place where kq is closed (Drop is called once)
            // - After drop, self is deallocated so no double-close is possible
            // Thread safety: Drop takes &mut self, ensuring exclusive access
            unsafe { close(self.kq) };
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::os::windows::io::RawSocket;

    pub type RawHandle = RawSocket;

    // WSAPoll constants
    const POLLIN: i16 = 0x0100;
    const POLLOUT: i16 = 0x0010;
    const POLLERR: i16 = 0x0001;
    const POLLHUP: i16 = 0x0002;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WSAPollFd {
        fd: RawSocket,
        events: i16,
        revents: i16,
    }

    #[repr(C)]
    struct WSAData {
        version: u16,
        high_version: u16,
        max_sockets: u16,
        max_udp_dg: u16,
        vendor_info: *mut i8,
        description: [i8; 257],
        system_status: [i8; 129],
    }

    /// # Safety
    ///
    /// These FFI functions directly call Windows Winsock2 API.
    /// Callers must ensure:
    /// - `WSAStartup` is called before any other Winsock functions
    /// - `fds` points to a valid array of `WSAPollFd` with at least `nfds` elements
    /// - `data` points to a valid `WSAData` structure
    /// - Sockets referenced in `fds` are valid and not closed while polling
    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAPoll(fds: *mut WSAPollFd, nfds: u32, timeout: i32) -> i32;
        fn WSAStartup(version: u16, data: *mut WSAData) -> i32;
        fn WSACleanup() -> i32;
        fn WSAGetLastError() -> i32;
    }

    struct FdEntry {
        fd: RawSocket,
        token: usize,
        interest: Interest,
    }

    pub struct Poller {
        entries: Vec<FdEntry>,
        initialized: bool,
    }

    impl Poller {
        pub fn new() -> io::Result<Self> {
            // Initialize Winsock
            // SAFETY: std::mem::zeroed() for WSAData is safe because:
            // - WSAData is a POD type with no invalid bit patterns
            // - All fields will be overwritten by WSAStartup
            let mut wsa_data: WSAData = unsafe { std::mem::zeroed() };
            // SAFETY: WSAStartup requires:
            // - version 0x0202 requests Winsock 2.2 (valid version)
            // - &mut wsa_data points to valid, writable WSAData structure
            // Error is checked immediately after the call
            // Thread safety: WSAStartup uses internal reference counting for initialization
            let ret = unsafe { WSAStartup(0x0202, &mut wsa_data) };
            if ret != 0 {
                return Err(io::Error::from_raw_os_error(ret));
            }

            Ok(Poller {
                entries: Vec::with_capacity(64),
                initialized: true,
            })
        }

        pub fn register(&mut self, fd: RawHandle, token: usize, interest: Interest) -> io::Result<()> {
            // Check if already registered
            if self.entries.iter().any(|e| e.fd == fd) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "fd already registered",
                ));
            }

            self.entries.push(FdEntry { fd, token, interest });
            Ok(())
        }

        pub fn modify(&mut self, fd: RawHandle, token: usize, interest: Interest) -> io::Result<()> {
            if let Some(entry) = self.entries.iter_mut().find(|e| e.fd == fd) {
                entry.token = token;
                entry.interest = interest;
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "fd not registered",
                ))
            }
        }

        pub fn deregister(&mut self, fd: RawHandle) -> io::Result<()> {
            if let Some(pos) = self.entries.iter().position(|e| e.fd == fd) {
                self.entries.swap_remove(pos);
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "fd not registered",
                ))
            }
        }

        pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<Vec<Event>> {
            if self.entries.is_empty() {
                // No fds to poll - sleep for timeout and return empty
                if let Some(dur) = timeout {
                    std::thread::sleep(dur);
                }
                return Ok(Vec::new());
            }

            let timeout_ms = timeout
                .map(|d| d.as_millis() as i32)
                .unwrap_or(-1);

            let mut pollfds: Vec<WSAPollFd> = self
                .entries
                .iter()
                .map(|e| WSAPollFd {
                    fd: e.fd,
                    events: interest_to_poll(&e.interest),
                    revents: 0,
                })
                .collect();

            loop {
                // SAFETY: WSAPoll requires:
                // - pollfds.as_mut_ptr() points to valid, writable WSAPollFd array
                // - pollfds.len() correctly reports the array length
                // - timeout_ms is a valid i32 (-1 for infinite, >=0 for milliseconds)
                // - All sockets in pollfds are valid (maintained by register/deregister)
                // Error is checked immediately
                // Thread safety: Poller requires &mut self, ensuring exclusive access
                let ret = unsafe { WSAPoll(pollfds.as_mut_ptr(), pollfds.len() as u32, timeout_ms) };

                if ret < 0 {
                    // SAFETY: WSAGetLastError() is safe to call after a failed Winsock call
                    // - No parameters required
                    // - Returns thread-local error code (no shared state issues)
                    let err = unsafe { WSAGetLastError() };
                    // WSAEINTR = 10004
                    if err == 10004 {
                        continue; // Retry on interrupt
                    }
                    return Err(io::Error::from_raw_os_error(err));
                }

                let mut result = Vec::new();
                for (i, pollfd) in pollfds.iter().enumerate() {
                    if pollfd.revents != 0 {
                        result.push(Event {
                            token: self.entries[i].token,
                            readable: (pollfd.revents & POLLIN) != 0,
                            writable: (pollfd.revents & POLLOUT) != 0,
                            error: (pollfd.revents & POLLERR) != 0,
                            hangup: (pollfd.revents & POLLHUP) != 0,
                        });
                    }
                }
                return Ok(result);
            }
        }
    }

    impl Drop for Poller {
        fn drop(&mut self) {
            if self.initialized {
                // SAFETY: WSACleanup is safe to call because:
                // - self.initialized is true only if WSAStartup succeeded
                // - This is the only place where WSACleanup is called (Drop is called once)
                // - WSACleanup uses reference counting; balances the WSAStartup call
                // Thread safety: Drop takes &mut self, ensuring exclusive access
                unsafe { WSACleanup() };
            }
        }
    }

    fn interest_to_poll(interest: &Interest) -> i16 {
        let mut events: i16 = 0;
        if interest.readable {
            events |= POLLIN;
        }
        if interest.writable {
            events |= POLLOUT;
        }
        events
    }
}

// ============================================================================
// Re-export platform-specific implementation
// ============================================================================

#[cfg(target_os = "linux")]
pub use linux::{Poller, RawHandle};

#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
pub use bsd::{Poller, RawHandle};

#[cfg(target_os = "windows")]
pub use windows::{Poller, RawHandle};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::io::Write;

    #[test]
    fn test_poller_basic() {
        let mut poller = Poller::new().expect("Failed to create poller");

        // Create a TCP pair for testing
        let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let client = TcpStream::connect(addr).expect("Failed to connect");
        client.set_nonblocking(true).expect("Failed to set nonblocking");
        let (mut server, _) = listener.accept().expect("Failed to accept");
        server.set_nonblocking(true).expect("Failed to set nonblocking");

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let client_fd = client.as_raw_fd();
            let server_fd = server.as_raw_fd();

            // Register client for readable
            poller.register(client_fd, 1, Interest::READABLE).expect("Failed to register");
            // Register server for writable
            poller.register(server_fd, 2, Interest::WRITABLE).expect("Failed to register");

            // Server should be immediately writable
            let events = poller.poll(Some(Duration::from_millis(100))).expect("Failed to poll");
            assert!(events.iter().any(|e| e.token == 2 && e.is_writable()));

            // Write some data from server
            server.write_all(b"hello").expect("Failed to write");

            // Client should become readable
            let events = poller.poll(Some(Duration::from_millis(100))).expect("Failed to poll");
            assert!(events.iter().any(|e| e.token == 1 && e.is_readable()));

            // Clean up
            poller.deregister(client_fd).expect("Failed to deregister");
            poller.deregister(server_fd).expect("Failed to deregister");
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawSocket;
            let client_fd = client.as_raw_socket();
            let server_fd = server.as_raw_socket();

            poller.register(client_fd, 1, Interest::READABLE).expect("Failed to register");
            poller.register(server_fd, 2, Interest::WRITABLE).expect("Failed to register");

            let events = poller.poll(Some(Duration::from_millis(100))).expect("Failed to poll");
            assert!(events.iter().any(|e| e.token == 2 && e.is_writable()));

            server.write_all(b"hello").expect("Failed to write");

            let events = poller.poll(Some(Duration::from_millis(100))).expect("Failed to poll");
            assert!(events.iter().any(|e| e.token == 1 && e.is_readable()));

            poller.deregister(client_fd).expect("Failed to deregister");
            poller.deregister(server_fd).expect("Failed to deregister");
        }
    }

    #[test]
    fn test_deregister_no_events() {
        let mut poller = Poller::new().expect("Failed to create poller");

        let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let client = TcpStream::connect(addr).expect("Failed to connect");
        client.set_nonblocking(true).expect("Failed to set nonblocking");

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = client.as_raw_fd();

            poller.register(fd, 1, Interest::READABLE).expect("Failed to register");
            poller.deregister(fd).expect("Failed to deregister");

            // After deregister, no events should be reported for this fd
            let events = poller.poll(Some(Duration::from_millis(50))).expect("Failed to poll");
            assert!(!events.iter().any(|e| e.token == 1));
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawSocket;
            let fd = client.as_raw_socket();

            poller.register(fd, 1, Interest::READABLE).expect("Failed to register");
            poller.deregister(fd).expect("Failed to deregister");

            let events = poller.poll(Some(Duration::from_millis(50))).expect("Failed to poll");
            assert!(!events.iter().any(|e| e.token == 1));
        }
    }

    #[test]
    fn test_modify_interest() {
        let mut poller = Poller::new().expect("Failed to create poller");

        let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let client = TcpStream::connect(addr).expect("Failed to connect");
        client.set_nonblocking(true).expect("Failed to set nonblocking");
        let (server, _) = listener.accept().expect("Failed to accept");
        server.set_nonblocking(true).expect("Failed to set nonblocking");

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let server_fd = server.as_raw_fd();

            // Register for readable only
            poller.register(server_fd, 1, Interest::READABLE).expect("Failed to register");

            // Modify to writable
            poller.modify(server_fd, 1, Interest::WRITABLE).expect("Failed to modify");

            // Should be writable now
            let events = poller.poll(Some(Duration::from_millis(100))).expect("Failed to poll");
            assert!(events.iter().any(|e| e.token == 1 && e.is_writable()));

            poller.deregister(server_fd).expect("Failed to deregister");
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawSocket;
            let server_fd = server.as_raw_socket();

            poller.register(server_fd, 1, Interest::READABLE).expect("Failed to register");
            poller.modify(server_fd, 1, Interest::WRITABLE).expect("Failed to modify");

            let events = poller.poll(Some(Duration::from_millis(100))).expect("Failed to poll");
            assert!(events.iter().any(|e| e.token == 1 && e.is_writable()));

            poller.deregister(server_fd).expect("Failed to deregister");
        }
    }
}
