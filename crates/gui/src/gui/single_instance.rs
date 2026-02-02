#[cfg(windows)]
mod windows_impl {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
    };

    const ERROR_ALREADY_EXISTS: u32 = 183;

    const MUTEX_NAME: &str = "Local\\codex-helper-gui-single-instance";
    const SHOW_EVENT_NAME: &str = "Local\\codex-helper-gui-show";

    #[derive(Debug)]
    pub enum AcquireResult {
        Primary(SingleInstance),
        SecondaryNotified,
    }

    #[derive(Debug)]
    pub struct SingleInstance {
        mutex: HANDLE,
        show_event: HANDLE,
    }

    impl SingleInstance {
        pub fn acquire_or_notify() -> anyhow::Result<AcquireResult> {
            unsafe {
                let mutex = CreateMutexW(std::ptr::null(), 0, wide_null(MUTEX_NAME).as_ptr());
                if mutex.is_null() {
                    anyhow::bail!("CreateMutexW failed: {}", GetLastError());
                }

                let already = GetLastError() == ERROR_ALREADY_EXISTS;
                if already {
                    CloseHandle(mutex);
                    notify_show();
                    return Ok(AcquireResult::SecondaryNotified);
                }

                let show_event =
                    CreateEventW(std::ptr::null(), 0, 0, wide_null(SHOW_EVENT_NAME).as_ptr());
                if show_event.is_null() {
                    let err = GetLastError();
                    CloseHandle(mutex);
                    anyhow::bail!("CreateEventW failed: {err}");
                }

                Ok(AcquireResult::Primary(SingleInstance { mutex, show_event }))
            }
        }

        pub fn check_show_requested(&self) -> bool {
            unsafe { WaitForSingleObject(self.show_event, 0) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for SingleInstance {
        fn drop(&mut self) {
            unsafe {
                if !self.show_event.is_null() {
                    CloseHandle(self.show_event);
                }
                if !self.mutex.is_null() {
                    CloseHandle(self.mutex);
                }
            }
        }
    }

    fn notify_show() {
        unsafe {
            let ev = OpenEventW(EVENT_MODIFY_STATE, 0, wide_null(SHOW_EVENT_NAME).as_ptr());
            if !ev.is_null() {
                let _ = SetEvent(ev);
                CloseHandle(ev);
            }
        }
    }

    fn wide_null(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(windows)]
pub use windows_impl::*;

#[cfg(not(windows))]
#[derive(Debug)]
pub enum AcquireResult {
    Primary(SingleInstance),
    SecondaryNotified,
}

#[cfg(not(windows))]
#[derive(Debug)]
pub struct SingleInstance;

#[cfg(not(windows))]
impl SingleInstance {
    pub fn acquire_or_notify() -> anyhow::Result<AcquireResult> {
        Ok(AcquireResult::Primary(SingleInstance))
    }

    pub fn check_show_requested(&self) -> bool {
        false
    }
}
