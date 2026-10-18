use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject};
use windows_sys::core::BOOL;

use crate::focus::image_path_of;
use crate::runtime::Command;
use crate::sdk_server::client::AcceptedClient;
use crate::sdk_server::listener::{CreateError, PipeFactory, PipeInstance, listen};
use crate::sdk_server::security::pipe_sddl;

pub const PIPE_NAME: &str = r"\\.\pipe\iCUESDKv4";
const PIPE_BUFFER: u32 = 64 * 1024;
const CLOSED_POLL: Duration = Duration::from_millis(100);

pub struct SdkListener {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl SdkListener {
    pub fn spawn(commands: Sender<Command>, enabled: Arc<AtomicBool>) -> io::Result<SdkListener> {
        let factory = NamedPipeFactory::new(PIPE_NAME)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::Builder::new()
            .name("sdk-listener".into())
            .spawn(move || listen(factory, commands, enabled, flag))?;
        Ok(SdkListener { stop, thread })
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.thread.join();
    }
}

pub struct NamedPipeFactory {
    name: Vec<u16>,
    descriptor: SecurityDescriptor,
}

impl NamedPipeFactory {
    pub fn new(name: &str) -> io::Result<Self> {
        let descriptor = SecurityDescriptor::from_sddl(&pipe_sddl(&current_user_sid()?))?;
        Ok(Self { name: wide(name), descriptor })
    }
}

impl PipeFactory for NamedPipeFactory {
    type Instance = NamedPipeInstance;

    fn create(&mut self, first: bool) -> Result<NamedPipeInstance, CreateError> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 0,
        };
        let first_instance = if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        // SAFETY: the name is NUL-terminated, and the attributes and the descriptor they point to outlive the call.
        let raw = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | first_instance,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER,
                PIPE_BUFFER,
                0,
                &attributes,
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            let error = io::Error::last_os_error();
            let taken =
                [ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY].map(|code| Some(code as i32)).contains(&error.raw_os_error());
            return Err(if first && taken {
                CreateError::InUse
            } else {
                CreateError::Failed(format!("CreateNamedPipeW: {error}"))
            });
        }
        // SAFETY: CreateNamedPipeW returned a valid handle that nothing else owns.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let event = new_event().map_err(|error| CreateError::Failed(format!("CreateEventW: {error}")))?;
        Ok(NamedPipeInstance {
            handle: Some(handle),
            event,
            connect: PendingConnect(Box::default()),
            connecting: false,
        })
    }
}

pub struct NamedPipeInstance {
    handle: Option<OwnedHandle>,
    event: OwnedHandle,
    connect: PendingConnect,
    connecting: bool,
}

struct PendingConnect(Box<OVERLAPPED>);

// SAFETY: the OVERLAPPED is only touched through the instance that owns it, and one thread holds an instance at a time.
unsafe impl Send for PendingConnect {}

impl NamedPipeInstance {
    fn raw(&self) -> HANDLE {
        self.handle.as_ref().map_or(INVALID_HANDLE_VALUE, AsRawHandle::as_raw_handle)
    }

    fn start_connect(&mut self) -> io::Result<bool> {
        *self.connect.0 = overlapped_on(&self.event);
        // SAFETY: the boxed OVERLAPPED stays at one address until the connect completes; Drop cancels and waits for it.
        if unsafe { ConnectNamedPipe(self.raw(), &mut *self.connect.0) } != 0 {
            return Ok(true);
        }
        match last_error() {
            ERROR_IO_PENDING => {
                self.connecting = true;
                Ok(false)
            }
            ERROR_PIPE_CONNECTED => Ok(true),
            ERROR_NO_DATA => {
                // SAFETY: resets the instance this struct owns after a client that left before being connected.
                unsafe { DisconnectNamedPipe(self.raw()) };
                Ok(false)
            }
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
}

impl PipeInstance for NamedPipeInstance {
    fn wait_for_client(&mut self, timeout: Duration) -> io::Result<bool> {
        if !self.connecting {
            if self.start_connect()? {
                return Ok(true);
            }
            if !self.connecting {
                return Ok(false);
            }
        }
        // SAFETY: waits on the event this instance owns.
        match unsafe { WaitForSingleObject(self.event.as_raw_handle(), millis(timeout)) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Ok(false),
            _ => return Err(io::Error::last_os_error()),
        }
        self.connecting = false;
        let mut transferred = 0;
        // SAFETY: the event is set, so the connect has completed and its OVERLAPPED is no longer in use.
        if unsafe { GetOverlappedResult(self.raw(), &*self.connect.0, &mut transferred, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(true)
    }

    fn accept(mut self) -> io::Result<AcceptedClient> {
        let handle =
            Arc::new(self.handle.take().ok_or_else(|| io::Error::other("the pipe instance was already accepted"))?);
        let exe = client_exe(&handle)?;
        let closed = Arc::new(AtomicBool::new(false));
        let reader = PipeReader(PipeHalf::new(&handle, &closed)?);
        let writer = PipeWriter(PipeHalf::new(&handle, &closed)?);
        let handle = Arc::downgrade(&handle);
        let hang_up = move || {
            closed.store(true, Ordering::SeqCst);
            if let Some(handle) = handle.upgrade() {
                // SAFETY: the upgraded Arc keeps the handle open for the call; each cancelled operation is waited for
                // by the half that started it before its OVERLAPPED is freed.
                unsafe { CancelIoEx(handle.as_raw_handle(), null()) };
            }
        };
        Ok(AcceptedClient { exe, reader: Box::new(reader), writer: Box::new(writer), hang_up: Arc::new(hang_up) })
    }
}

impl Drop for NamedPipeInstance {
    fn drop(&mut self) {
        if !self.connecting {
            return;
        }
        let mut transferred = 0;
        // SAFETY: cancels the pending connect and waits for it to end, so the OVERLAPPED is unused when it is freed.
        unsafe {
            CancelIoEx(self.raw(), &*self.connect.0);
            GetOverlappedResult(self.raw(), &*self.connect.0, &mut transferred, 1);
        }
    }
}

struct PipeHalf {
    handle: Arc<OwnedHandle>,
    event: OwnedHandle,
    closed: Arc<AtomicBool>,
}

impl PipeHalf {
    fn new(handle: &Arc<OwnedHandle>, closed: &Arc<AtomicBool>) -> io::Result<Self> {
        Ok(Self { handle: handle.clone(), event: new_event()?, closed: closed.clone() })
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn transfer(&self, start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> BOOL) -> io::Result<u32> {
        let handle = self.handle.as_raw_handle();
        let mut overlapped = overlapped_on(&self.event);
        if start(handle, &mut overlapped) == 0 {
            let code = last_error();
            if code != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
        }
        self.finish(handle, &overlapped)
    }

    fn finish(&self, handle: HANDLE, overlapped: &OVERLAPPED) -> io::Result<u32> {
        loop {
            // SAFETY: waits on the event this half owns.
            match unsafe { WaitForSingleObject(overlapped.hEvent, millis(CLOSED_POLL)) } {
                WAIT_OBJECT_0 => break,
                WAIT_TIMEOUT if !self.closed() => {}
                _ => {
                    // SAFETY: cancels only this half's operation; the result call below waits for it to end.
                    unsafe { CancelIoEx(handle, overlapped) };
                    break;
                }
            }
        }
        let mut transferred = 0;
        // SAFETY: waits until the operation has ended, after which its OVERLAPPED and buffer are no longer in use.
        if unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(transferred)
    }
}

struct PipeReader(PipeHalf);

impl Read for PipeReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let length = buffer.len().min(u32::MAX as usize) as u32;
        while !self.0.closed() {
            let result = self.0.transfer(|handle, overlapped| {
                // SAFETY: transfer returns only after the read has ended, so the buffer and OVERLAPPED outlive it.
                unsafe { ReadFile(handle, buffer.as_mut_ptr(), length, null_mut(), overlapped) }
            });
            match result {
                Ok(0) => continue,
                Ok(read) => return Ok(read as usize),
                Err(error) if self.0.closed() || ends_stream(&error) => return Ok(0),
                Err(error) => return Err(error),
            }
        }
        Ok(0)
    }
}

struct PipeWriter(PipeHalf);

impl Write for PipeWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let length = buffer.len().min(u32::MAX as usize) as u32;
        let written = self.0.transfer(|handle, overlapped| {
            // SAFETY: transfer returns only after the write has ended, so the buffer and OVERLAPPED outlive it.
            unsafe { WriteFile(handle, buffer.as_ptr(), length, null_mut(), overlapped) }
        })?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn ends_stream(error: &io::Error) -> bool {
    [ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED, ERROR_NO_DATA]
        .map(|code| Some(code as i32))
        .contains(&error.raw_os_error())
}

fn client_exe(handle: &OwnedHandle) -> io::Result<String> {
    let mut pid = 0;
    // SAFETY: queries a connected pipe handle that the caller keeps open.
    if unsafe { GetNamedPipeClientProcessId(handle.as_raw_handle(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let file_name =
        image_path_of(pid).and_then(|path| Some(Path::new(&path).file_name()?.to_string_lossy().into_owned()));
    Ok(file_name.unwrap_or_else(|| format!("pid {pid}")))
}

fn current_user_sid() -> io::Result<String> {
    let mut raw = null_mut();
    // SAFETY: opens this process's own token for querying; the handle is checked before it is owned.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned a valid handle that nothing else owns.
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut length = 0;
    // SAFETY: a size query without a buffer; it fails with ERROR_INSUFFICIENT_BUFFER and reports the length.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length) };
    let mut buffer = vec![0u64; (length as usize).div_ceil(size_of::<u64>())];
    // SAFETY: the buffer is at least `length` bytes and 8-byte aligned, which TOKEN_USER needs.
    let filled = unsafe {
        GetTokenInformation(token.as_raw_handle(), TokenUser, buffer.as_mut_ptr().cast(), length, &mut length)
    };
    if filled == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut text = null_mut();
    // SAFETY: the buffer holds a TOKEN_USER whose SID lies in the same live buffer; the string is freed with LocalFree.
    unsafe {
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let length = (0..).take_while(|&index| *text.add(index) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        Ok(sid)
    }
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is never written after creation and is freed only on drop, so it may move between threads.
unsafe impl Send for SecurityDescriptor {}

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let mut descriptor = null_mut();
        // SAFETY: the SDDL text is NUL-terminated for the call; the descriptor is owned by Self and freed with LocalFree.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(sddl).as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: frees the descriptor ConvertStringSecurityDescriptorToSecurityDescriptorW allocated; nothing uses it after.
        unsafe { LocalFree(self.0) };
    }
}

fn new_event() -> io::Result<OwnedHandle> {
    // SAFETY: creates an unnamed manual-reset event; the handle is checked before it is owned.
    let raw = unsafe { CreateEventW(null(), 1, 0, null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateEventW returned a valid handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn overlapped_on(event: &OwnedHandle) -> OVERLAPPED {
    OVERLAPPED { hEvent: event.as_raw_handle(), ..OVERLAPPED::default() }
}

fn last_error() -> u32 {
    io::Error::last_os_error().raw_os_error().unwrap_or_default() as u32
}

fn millis(duration: Duration) -> u32 {
    duration.as_millis().min(u128::from(u32::MAX - 1)) as u32
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::runtime::Command;
    use crate::sdk_server::SdkCommand;
    use crate::sdk_server::listener::{CreateError, PipeFactory, PipeInstance, listen};

    const HANDSHAKE: &[u8] = br#"{"method":0,"code":-1}"#;
    const PATIENCE: Duration = Duration::from_secs(5);

    fn test_pipe(test: &str) -> String {
        format!(r"\\.\pipe\k100d-sdk-test-{test}-{}", std::process::id())
    }

    fn current_exe_file_name() -> String {
        std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned()
    }

    fn open_when_listening(name: &str) -> File {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match OpenOptions::new().read(true).write(true).open(name) {
                Ok(pipe) => return pipe,
                Err(error) if Instant::now() > deadline => panic!("could not open {name}: {error}"),
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    fn next_sdk(receiver: &Receiver<Command>) -> Command {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(left) {
                Ok(Command::Sdk(SdkCommand::Status(_))) => {}
                Ok(command) => return command,
                Err(error) => panic!("no SDK command within {PATIENCE:?}: {error}"),
            }
        }
    }

    struct Serving {
        commands: Receiver<Command>,
        stop: Arc<AtomicBool>,
        thread: JoinHandle<()>,
    }

    impl Serving {
        fn start(name: &str) -> Self {
            let (sender, commands) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let factory = NamedPipeFactory::new(name).unwrap();
            let (enabled, flag) = (Arc::new(AtomicBool::new(true)), stop.clone());
            let thread = thread::spawn(move || listen(factory, sender, enabled, flag));
            Self { commands, stop, thread }
        }

        fn finish(self) {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.join().unwrap();
        }
    }

    #[test]
    fn a_second_server_on_the_same_name_is_in_use() {
        let name = test_pipe("in-use");
        let mut first = NamedPipeFactory::new(&name).unwrap();
        let _held = first.create(true).unwrap();
        assert!(matches!(NamedPipeFactory::new(&name).unwrap().create(true), Err(CreateError::InUse)));
    }

    #[test]
    fn a_client_round_trips_a_frame_and_is_named_by_its_exe() {
        let name = test_pipe("round-trip");
        let serving = Serving::start(&name);
        let mut pipe = open_when_listening(&name);
        pipe.write_all(&sdk::encode_frame(HANDSHAKE)).unwrap();
        let Command::Sdk(SdkCommand::Connected { exe, outbox, .. }) = next_sdk(&serving.commands) else {
            panic!("expected a connection")
        };
        assert_eq!(exe, current_exe_file_name());
        let Command::Sdk(SdkCommand::Message { payload, .. }) = next_sdk(&serving.commands) else {
            panic!("expected a message")
        };
        assert_eq!(payload, HANDSHAKE);
        outbox.send(b"{}".to_vec()).unwrap();
        let mut reply = [0u8; 6];
        pipe.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [0, 0, 0, 2, b'{', b'}']);
        drop(pipe);
        assert!(matches!(next_sdk(&serving.commands), Command::Sdk(SdkCommand::Disconnected { .. })));
        serving.finish();
    }

    #[test]
    fn a_frame_split_across_writes_is_answered_and_the_reply_arrives_whole() {
        let name = test_pipe("split");
        let serving = Serving::start(&name);
        let mut pipe = open_when_listening(&name);
        let frame = sdk::encode_frame(HANDSHAKE);
        pipe.write_all(&frame[..4]).unwrap();
        let Command::Sdk(SdkCommand::Connected { outbox, .. }) = next_sdk(&serving.commands) else {
            panic!("expected a connection")
        };
        thread::sleep(Duration::from_millis(200));
        pipe.write_all(&frame[4..10]).unwrap();
        thread::sleep(Duration::from_millis(200));
        assert!(serving.commands.try_recv().is_err(), "nothing is forwarded before the frame is complete");
        pipe.write_all(&frame[10..]).unwrap();
        let Command::Sdk(SdkCommand::Message { payload, .. }) = next_sdk(&serving.commands) else {
            panic!("expected a message")
        };
        assert_eq!(payload, HANDSHAKE);
        outbox.send(b"{}".to_vec()).unwrap();
        let mut reply = [0u8; 64];
        let read = pipe.read(&mut reply).unwrap();
        assert_eq!(reply[..read], [0, 0, 0, 2, b'{', b'}'], "prefix and body come in one write");
        serving.finish();
    }

    #[test]
    fn closing_the_outbox_hangs_up_after_the_last_reply() {
        let name = test_pipe("hang-up");
        let serving = Serving::start(&name);
        let mut pipe = open_when_listening(&name);
        let Command::Sdk(SdkCommand::Connected { outbox, .. }) = next_sdk(&serving.commands) else {
            panic!("expected a connection")
        };
        outbox.send(b"bye".to_vec()).unwrap();
        drop(outbox);
        assert!(matches!(next_sdk(&serving.commands), Command::Sdk(SdkCommand::Disconnected { .. })));
        let mut received = Vec::new();
        pipe.read_to_end(&mut received).unwrap();
        assert_eq!(received, sdk::encode_frame(b"bye"));
        serving.finish();
    }

    #[test]
    fn a_client_that_never_reads_is_hung_up_while_the_writer_is_blocked() {
        let name = test_pipe("never-reads");
        let serving = Serving::start(&name);
        let mut pipe = open_when_listening(&name);
        let Command::Sdk(SdkCommand::Connected { outbox, hang_up, .. }) = next_sdk(&serving.commands) else {
            panic!("expected a connection")
        };
        for _ in 0..4 {
            outbox.send(vec![b'x'; 256 * 1024]).unwrap();
        }
        thread::sleep(Duration::from_millis(300));
        drop(outbox);
        hang_up();
        drop(hang_up);
        assert!(matches!(next_sdk(&serving.commands), Command::Sdk(SdkCommand::Disconnected { .. })));
        let deadline = Instant::now() + PATIENCE;
        while pipe.write_all(&sdk::encode_frame(HANDSHAKE)).is_ok() {
            assert!(Instant::now() < deadline, "the server end of the pipe stayed open");
            thread::sleep(Duration::from_millis(20));
        }
        serving.finish();
    }

    #[test]
    fn an_unanswered_instance_lets_go_of_the_name_and_a_waiting_client_is_taken() {
        let name = test_pipe("idle");
        let mut factory = NamedPipeFactory::new(&name).unwrap();
        let mut idle = factory.create(true).unwrap();
        assert!(!idle.wait_for_client(Duration::from_millis(50)).unwrap());
        assert!(!idle.wait_for_client(Duration::from_millis(50)).unwrap());
        drop(idle);
        let mut instance = factory.create(true).unwrap();
        let _pipe = open_when_listening(&name);
        assert!(instance.wait_for_client(PATIENCE).unwrap());
        assert_eq!(instance.accept().unwrap().exe, current_exe_file_name());
    }
}
