//! Keep native Core ML diagnostics out of headless JSON results.

pub struct NativeStdoutRedirect {
    saved_stdout: libc::c_int,
}

impl NativeStdoutRedirect {
    /// Used only by headless file transcription, while no JSON is being emitted.
    pub fn to_stderr() -> std::io::Result<Self> {
        // Native frameworks use C stdout rather than Rust's logging facade.
        // Flush before switching descriptors and retain a descriptor to restore.
        unsafe {
            libc::fflush(std::ptr::null_mut());
            let saved_stdout = libc::dup(libc::STDOUT_FILENO);
            if saved_stdout < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
                let error = std::io::Error::last_os_error();
                libc::close(saved_stdout);
                return Err(error);
            }
            Ok(Self { saved_stdout })
        }
    }
}

impl Drop for NativeStdoutRedirect {
    fn drop(&mut self) {
        unsafe {
            libc::fflush(std::ptr::null_mut());
            libc::dup2(self.saved_stdout, libc::STDOUT_FILENO);
            libc::close(self.saved_stdout);
        }
    }
}
