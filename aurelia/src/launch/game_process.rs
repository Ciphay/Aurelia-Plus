use std::io;
use std::process::ExitStatus;

pub enum GameProcess {
    Child(std::process::Child),
    #[cfg(windows)]
    Elevated(elevated::ElevatedProcess),
}

impl From<std::process::Child> for GameProcess {
    fn from(child: std::process::Child) -> Self {
        GameProcess::Child(child)
    }
}

impl GameProcess {
    pub fn id(&self) -> u32 {
        match self {
            GameProcess::Child(child) => child.id(),
            #[cfg(windows)]
            GameProcess::Elevated(process) => process.id(),
        }
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        match self {
            GameProcess::Child(child) => child.wait(),
            #[cfg(windows)]
            GameProcess::Elevated(process) => process.wait(),
        }
    }
}

pub const ERROR_ELEVATION_REQUIRED: i32 = 740;

pub fn needs_elevation(err: &io::Error) -> bool {
    err.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED)
}

pub fn windows_command_line<S: AsRef<str>>(args: &[S]) -> String {
    let mut out = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        push_quoted(&mut out, arg.as_ref());
    }
    out
}

fn push_quoted(out: &mut String, arg: &str) {
    let needs_quotes = arg.is_empty() || arg.contains([' ', '\t', '\n', '\x0b', '"']);
    if !needs_quotes {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // escape backslashes, then the quote
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // double before the closing quote
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

#[cfg(windows)]
pub mod elevated {
    use super::windows_command_line;
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::ExitStatusExt;
    use std::path::Path;
    use std::process::ExitStatus;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessId, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    pub struct ElevatedProcess {
        handle: HANDLE,
        pid: u32,
    }

    unsafe impl Send for ElevatedProcess {}

    impl ElevatedProcess {
        pub fn id(&self) -> u32 {
            self.pid
        }

        pub fn wait(&mut self) -> io::Result<ExitStatus> {
            unsafe {
                if WaitForSingleObject(self.handle, INFINITE) != WAIT_OBJECT_0 {
                    return Err(io::Error::last_os_error());
                }
                let mut code = 0u32;
                if GetExitCodeProcess(self.handle, &mut code) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(ExitStatus::from_raw(code))
            }
        }
    }

    impl Drop for ElevatedProcess {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn spawn(exe: &Path, args: &[String], cwd: &Path) -> io::Result<ElevatedProcess> {
        let verb = wide(OsStr::new("runas"));
        let file = wide(exe.as_os_str());
        let params = wide(OsStr::new(&windows_command_line(args)));
        let dir = wide(cwd.as_os_str());

        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.lpDirectory = dir.as_ptr();
        info.nShow = SW_SHOWNORMAL;

        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_CANCELLED as i32) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "administrator prompt was declined",
                ));
            }
            return Err(err);
        }
        if info.hProcess.is_null() {
            return Err(io::Error::other("elevated launch returned no process"));
        }
        let pid = unsafe { GetProcessId(info.hProcess) };
        Ok(ElevatedProcess {
            handle: info.hProcess,
            pid,
        })
    }
}
