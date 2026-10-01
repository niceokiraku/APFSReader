//! Named-pipe transport for the disk helper (Windows).
//!
//! The server side is used by the elevated broker, the client side by the
//! normal-rights app. The pipe is created with an access list naming only the
//! launching user, refuses connections from other machines and accepts a
//! single client, who must then present the token (see the parent module).

use super::{connect, serve_stream, RemoteSource, TOKEN_LEN};
use crate::device::BlockSource;
use crate::{Error, Result};
use std::fs::{File, OpenOptions};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::time::{Duration, Instant};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, HANDLE, HLOCAL,
    INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, SetNamedPipeHandleState, PIPE_NOWAIT, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

const PIPE_BUFFER: u32 = 1 << 20;

/// Cryptographically random bytes from the system generator.
pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    unsafe {
        let status = BCryptGenRandom(None, &mut out, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
        if status.0 != 0 {
            return Err(Error::Io(std::io::Error::other("the system random number generator failed")));
        }
    }
    Ok(out)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// A fresh pipe name nobody can predict.
pub fn new_pipe_name() -> Result<String> {
    Ok(format!("apfsreader-{}", to_hex(&random_bytes::<12>()?)))
}

/// The current user's SID in string form (`S-1-5-21-...`).
pub fn current_user_sid() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        let mut buf = vec![0u8; needed.max(64) as usize];
        let result = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), buf.len() as u32, &mut needed);
        let _ = CloseHandle(token);
        result.map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        let user = &*(buf.as_ptr().cast::<TOKEN_USER>());
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s)
            .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        let out = s.to_string().map_err(|_| Error::Format("unreadable SID"))?;
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        Ok(out)
    }
}

/// Create the pipe, wait up to `connect_timeout` for the one client, then
/// serve `src` to it until it disconnects. `allowed_sid` is the only account
/// that may open the pipe.
pub fn serve_pipe(
    name: &str,
    allowed_sid: &str,
    token: &[u8; TOKEN_LEN],
    src: &dyn BlockSource,
    connect_timeout: Duration,
) -> Result<()> {
    let sddl = HSTRING::from(format!("D:(A;;GA;;;{allowed_sid})"));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut descriptor, None)
            .map_err(|e| Error::Io(std::io::Error::other(format!("bad access list: {e}"))))?;
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let full = HSTRING::from(format!(r"\\.\pipe\{name}"));
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(full.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            Some(&attributes),
        )
    };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }

    // Wait for the client without blocking, so a client that never shows up
    // cannot leave an elevated process hanging (closing a handle does not
    // reliably wake a blocked ConnectNamedPipe).
    let deadline = Instant::now() + connect_timeout;
    loop {
        let code = match unsafe { ConnectNamedPipe(handle, None) } {
            Ok(()) => break,
            Err(e) => e.code(),
        };
        if code == ERROR_PIPE_CONNECTED.to_hresult() {
            break;
        }
        let waiting = code == ERROR_PIPE_LISTENING.to_hresult() || code == ERROR_IO_PENDING.to_hresult();
        if !waiting || Instant::now() >= deadline {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "no client connected to the disk helper",
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Connected: switch to ordinary blocking reads and writes.
    unsafe {
        let mode = PIPE_WAIT;
        SetNamedPipeHandleState(handle, Some(&mode), None, None)
            .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
    }

    // SAFETY: the handle is a valid, exclusively owned pipe handle; the File takes ownership.
    let mut pipe = unsafe { File::from_raw_handle(handle.0 as RawHandle) };
    serve_stream(&mut pipe, src, token)?;
    Ok(())
}

/// Connect to a broker's pipe, retrying while it starts up.
pub fn connect_pipe(name: &str, token: &[u8; TOKEN_LEN], wait: Duration) -> Result<RemoteSource<File>> {
    let path = format!(r"\\.\pipe\{name}");
    let deadline = Instant::now() + wait;
    loop {
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => return connect(file, token),
            Err(e) if Instant::now() < deadline => {
                // Not created yet (file not found) or momentarily busy.
                let _ = e;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(Error::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Mem(Vec<u8>);
    impl BlockSource for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let end = offset as usize + buf.len();
            if end > self.0.len() {
                return Err(Error::OutOfRange { offset, len: buf.len() });
            }
            buf.copy_from_slice(&self.0[offset as usize..end]);
            Ok(())
        }
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        let b = [0u8, 1, 0xAB, 0xFF];
        assert_eq!(from_hex::<4>(&to_hex(&b)), Some(b));
        assert_eq!(from_hex::<4>("zz"), None);
        assert_eq!(from_hex::<2>("0g00"), None);
    }

    #[test]
    fn random_values_differ() {
        assert_ne!(random_bytes::<16>().unwrap(), random_bytes::<16>().unwrap());
    }

    #[test]
    fn the_current_user_has_a_sid() {
        assert!(current_user_sid().unwrap().starts_with("S-1-"));
    }

    #[test]
    fn a_real_named_pipe_serves_and_authenticates() {
        let data: Vec<u8> = (0..512 * 10_000u32).map(|i| (i % 253) as u8).collect();
        let name = new_pipe_name().unwrap();
        let token = random_bytes::<TOKEN_LEN>().unwrap();
        let sid = current_user_sid().unwrap();
        let served = Arc::new(Mem(data.clone()));
        let (n2, s2, sv) = (name.clone(), sid.clone(), served.clone());
        let server = std::thread::spawn(move || serve_pipe(&n2, &s2, &token, &*sv, Duration::from_secs(10)));

        let remote = connect_pipe(&name, &token, Duration::from_secs(10)).unwrap();
        assert_eq!(remote.len(), data.len() as u64);
        let mut buf = vec![0u8; 3_000_000];
        remote.read_at(12_345, &mut buf).unwrap();
        assert_eq!(&buf[..], &data[12_345..12_345 + 3_000_000]);
        drop(remote);
        server.join().unwrap().unwrap();
    }

    #[test]
    fn a_wrong_token_is_refused_over_a_real_pipe() {
        let name = new_pipe_name().unwrap();
        let token = random_bytes::<TOKEN_LEN>().unwrap();
        let sid = current_user_sid().unwrap();
        let (n2, s2) = (name.clone(), sid.clone());
        let server = std::thread::spawn(move || {
            serve_pipe(&n2, &s2, &token, &Mem(vec![0u8; 4096]), Duration::from_secs(10))
        });
        let wrong = random_bytes::<TOKEN_LEN>().unwrap();
        assert!(connect_pipe(&name, &wrong, Duration::from_secs(10)).is_err());
        server.join().unwrap().unwrap();
    }

    #[test]
    fn a_server_nobody_connects_to_gives_up() {
        let name = new_pipe_name().unwrap();
        let token = random_bytes::<TOKEN_LEN>().unwrap();
        let sid = current_user_sid().unwrap();
        let started = Instant::now();
        let r = serve_pipe(&name, &sid, &token, &Mem(vec![0u8; 512]), Duration::from_millis(400));
        assert!(r.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

/// The elevated helper process, as seen from the app.
pub struct BrokerProcess {
    handle: HANDLE,
}

// SAFETY: a process handle may be used from any thread.
unsafe impl Send for BrokerProcess {}
// SAFETY: likewise; the handle is only queried and closed.
unsafe impl Sync for BrokerProcess {}

impl BrokerProcess {
    /// `None` while the helper is still running, otherwise its exit code.
    pub fn exit_code(&self) -> Option<u32> {
        let mut code = 0u32;
        unsafe {
            if GetExitCodeProcess(self.handle, &mut code).is_err() {
                return None;
            }
        }
        // STILL_ACTIVE
        (code != 259).then_some(code)
    }
}

impl Drop for BrokerProcess {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Windows' "the user declined the elevation prompt".
const ERROR_CANCELLED: i32 = 1223;

/// True if `e` is the user pressing "No" on the UAC prompt.
pub fn is_user_cancelled(e: &Error) -> bool {
    matches!(e, Error::Io(io) if io.raw_os_error() == Some(ERROR_CANCELLED))
}

/// Start the broker with administrator rights (this shows the UAC prompt
/// unless the app is already elevated). It serves `disk` on `pipe` to `sid`.
pub fn launch_broker(
    exe: &std::path::Path,
    disk: u32,
    pipe: &str,
    token: &[u8; TOKEN_LEN],
    sid: &str,
    timeout_secs: u32,
) -> Result<BrokerProcess> {
    let params = HSTRING::from(format!(
        "--disk {disk} --pipe {pipe} --token {} --sid {sid} --timeout {timeout_secs}",
        to_hex(token)
    ));
    let file = HSTRING::from(exe.as_os_str());
    let verb = HSTRING::from("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: 0, // SW_HIDE: the helper has no window
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| {
        let code = (e.code().0 & 0xFFFF) as i32;
        Error::Io(std::io::Error::from_raw_os_error(code))
    })?;
    if info.hProcess.is_invalid() {
        return Err(Error::Io(std::io::Error::other("the disk helper did not start")));
    }
    Ok(BrokerProcess { handle: info.hProcess })
}
