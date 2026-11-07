//! Windows named pipe: `\\.\pipe\aivory-code-<name>`. Byte mode, remote clients rejected.
//!
//! The first pipe instance uses `FILE_FLAG_FIRST_PIPE_INSTANCE`, so that no other process can
//! already own the name. The listener always keeps one unconnected instance ready.
//!
//! Both ends open the pipe for overlapped I/O. Windows serializes all I/O on a synchronous
//! handle, so a read that waits for the next request would block a notification write from
//! another thread. With overlapped I/O each direction waits on its own event.
#![allow(unsafe_code)]

use std::fs::OpenOptions;
use std::io::{self, BufReader, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::CreateEventW;

use super::Connection;

const BUFFER_BYTES: u32 = 64 * 1024;
const ERROR_PIPE_BUSY: i32 = 231;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const CONNECT_RETRY: Duration = Duration::from_secs(3);

pub fn address(name: &str) -> String {
    format!(r"\\.\pipe\aivory-code-{name}")
}

#[derive(Debug)]
pub struct Listener {
    name: String,
    pending: Option<OwnedHandle>,
}

impl Listener {
    pub fn bind(name: &str) -> io::Result<Self> {
        let pending = create_instance(&address(name), true)?;
        Ok(Self {
            name: name.to_string(),
            pending: Some(pending),
        })
    }

    pub fn accept(&mut self) -> io::Result<Connection> {
        let pipe = match self.pending.take() {
            Some(p) => p,
            None => create_instance(&address(&self.name), false)?,
        };
        let pipe = Arc::new(pipe);
        connect_instance(&PipeIo::new(Arc::clone(&pipe))?)?;
        // Keep a listening instance ready for the next client.
        self.pending = Some(create_instance(&address(&self.name), false)?);
        connection(pipe)
    }
}

fn connection(pipe: Arc<OwnedHandle>) -> io::Result<Connection> {
    Ok(Connection {
        reader: BufReader::new(Box::new(PipeIo::new(Arc::clone(&pipe))?)),
        writer: Box::new(PipeIo::new(pipe)?),
    })
}

fn create_instance(path: &str, first: bool) -> io::Result<OwnedHandle> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let open_mode = PIPE_ACCESS_DUPLEX
        | FILE_FLAG_OVERLAPPED
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that lives for the call; the security
    // attributes pointer may be null (default descriptor); the result is checked below.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER_BYTES,
            BUFFER_BYTES,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `handle` is a valid pipe handle that nothing else owns; `OwnedHandle` takes it.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
}

fn connect_instance(pipe: &PipeIo) -> io::Result<()> {
    // SAFETY: `wait` passes the valid pipe handle and an OVERLAPPED that outlives the call.
    match pipe.wait(|h, ov| unsafe { ConnectNamedPipe(h, ov) }) {
        Ok(_) => Ok(()),
        // The client connected between create and connect.
        Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn connect(name: &str) -> io::Result<Connection> {
    let path = address(name);
    let started = Instant::now();
    loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&path)
        {
            Ok(pipe) => return connection(Arc::new(OwnedHandle::from(pipe))),
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(ERROR_PIPE_BUSY | ERROR_FILE_NOT_FOUND)
                ) && started.elapsed() < CONNECT_RETRY =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

/// One direction of an overlapped pipe handle, with blocking `Read` and `Write`.
#[derive(Debug)]
struct PipeIo {
    pipe: Arc<OwnedHandle>,
    /// Manual-reset event of this direction.
    event: OwnedHandle,
}

impl PipeIo {
    fn new(pipe: Arc<OwnedHandle>) -> io::Result<Self> {
        // SAFETY: null attributes and name are valid; the result is checked below.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `event` is a new event handle that nothing else owns.
        let event = unsafe { OwnedHandle::from_raw_handle(event as RawHandle) };
        Ok(Self { pipe, event })
    }

    /// Start one overlapped call and wait until it completes. Returns the bytes moved.
    fn wait(&self, start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> i32) -> io::Result<u32> {
        // SAFETY: OVERLAPPED is plain data; all zero is its documented initial state.
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = self.event.as_raw_handle() as HANDLE;
        let handle = self.pipe.as_raw_handle() as HANDLE;
        if start(handle, &mut ov) == 0 {
            // SAFETY: plain thread-local error query.
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
        }
        let mut moved = 0u32;
        // SAFETY: `ov` stays alive and unmoved until the wait returns; bWait = TRUE blocks until
        // the operation is complete, so the kernel no longer uses `ov` or the buffer after it.
        if unsafe { GetOverlappedResult(handle, &ov, &mut moved, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(moved)
    }
}

fn is_closed(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(c) if c == ERROR_BROKEN_PIPE as i32 || c == ERROR_PIPE_NOT_CONNECTED as i32)
}

impl Read for PipeIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;
        let ptr = buf.as_mut_ptr();
        // SAFETY: `ptr` points to `len` writable bytes that outlive the wait.
        match self.wait(|h, ov| unsafe { ReadFile(h, ptr, len, std::ptr::null_mut(), ov) }) {
            Ok(n) => Ok(n as usize),
            Err(e) if is_closed(&e) => Ok(0),
            Err(e) => Err(e),
        }
    }
}

impl Write for PipeIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize) as u32;
        let ptr = buf.as_ptr();
        // SAFETY: `ptr` points to `len` readable bytes that outlive the wait.
        self.wait(|h, ov| unsafe { WriteFile(h, ptr, len, std::ptr::null_mut(), ov) })
            .map(|n| n as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
