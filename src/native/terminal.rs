//! A terminal joins the running Application's cgroup through the N1 launcher.
//! Closing it signals only its marked processes, never the Application tree.

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::CommandExt;
    use std::sync::Arc;

    use anyhow::{Context, Result};
    use tokio::sync::mpsc;

    use crate::native::{ApplicationCgroup, LaunchRequest};
    use crate::terminal::{Input, Output, Session, Size};

    #[derive(Debug, Clone)]
    struct Cleanup {
        cgroup: ApplicationCgroup,
        uid: u32,
        marker: String,
    }

    impl Cleanup {
        fn run(&self) {
            // A pidfd binds the signal to the inspected process even if its
            // numeric PID is recycled. The random marker is inherited by
            // ordinary and detached terminal children, not the main service.
            for _ in 0..3 {
                for pid in self.cgroup.processes().unwrap_or_default() {
                    // SAFETY: pidfd_open receives integers and returns an fd.
                    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
                    if fd < 0 {
                        continue;
                    }
                    // SAFETY: successful pidfd_open returns a new owned fd.
                    let process = unsafe { File::from_raw_fd(fd) };
                    let path = std::path::PathBuf::from(format!("/proc/{pid}"));
                    if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.uid() == self.uid) {
                        continue;
                    }
                    let Ok(environment) = std::fs::read(path.join("environ")) else {
                        continue;
                    };
                    if !environment
                        .split(|byte| *byte == 0)
                        .any(|entry| entry == self.marker.as_bytes())
                    {
                        continue;
                    }
                    // SAFETY: the live pidfd identifies only this session's
                    // checked process. No pointer argument is dereferenced.
                    unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            process.as_raw_fd(),
                            libc::SIGKILL,
                            std::ptr::null::<libc::siginfo_t>(),
                            0,
                        );
                    }
                }
            }
        }
    }

    #[derive(Debug, Clone)]
    struct Killer(Arc<Cleanup>);
    impl portable_pty::ChildKiller for Killer {
        fn kill(&mut self) -> std::io::Result<()> {
            self.0.run();
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(self.clone())
        }
    }

    fn resize(file: &File, size: Size) -> std::io::Result<()> {
        let dimensions = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: the PTY is open and dimensions points to a winsize value.
        if unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &dimensions) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn open(
        cgroup: &ApplicationCgroup,
        mut request: LaunchRequest,
        size: Size,
    ) -> Result<Session> {
        let account = super::super::resolve(&request.account)?;
        let marker = format!("SF_TERMINAL_SESSION={:032x}", rand::random::<u128>());
        let (key, value) = marker.split_once('=').unwrap();
        request
            .environment
            .retain(|(name, _)| name != key && name != "TERM");
        request.environment.push((key.into(), value.into()));
        request
            .environment
            .push(("TERM".into(), "xterm-256color".into()));
        let mut command = super::super::launch::command_in(cgroup, &request)?;
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: both output pointers are valid. Null uses default settings.
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: openpty gave ownership of these distinct descriptors.
        let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        for fd in [&master, &slave] {
            // SAFETY: both descriptors are live; set close-on-exec so only
            // the explicit stdin/stdout/stderr duplicates reach the shell.
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        resize(&master, size)?;
        let mut reader = master.try_clone()?;
        let mut writer = master.try_clone()?;
        command
            .stdin(slave.try_clone()?)
            .stdout(slave.try_clone()?)
            .stderr(slave);
        // SAFETY: raw, allocation-free calls after the N1 privilege drop and
        // before shell exec. The shell becomes the controlling PTY session.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .context("could not start the Application terminal")?;
        drop(command);
        let cleanup = Arc::new(Cleanup {
            cgroup: cgroup.clone(),
            uid: account.uid,
            marker,
        });
        let (input, mut inputs) = mpsc::channel::<Input>(32);
        let (output_tx, output) = mpsc::channel(32);
        let errors = output_tx.clone();
        std::thread::spawn(move || {
            while let Some(input) = inputs.blocking_recv() {
                let result = match input {
                    Input::Input { data } => writer.write_all(data.as_bytes()),
                    Input::Resize { size } => resize(&master, size),
                };
                if result.is_err() {
                    let _ = errors
                        .blocking_send(Output::Error("The Application terminal closed.".into()));
                    break;
                }
            }
        });
        let reader_tx = output_tx.clone();
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if reader_tx
                            .blocking_send(Output::Data(buffer[..count].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
        let finished = cleanup.clone();
        std::thread::spawn(move || {
            let status = child.wait();
            finished.run();
            let _ = match status {
                Ok(status) => {
                    output_tx.blocking_send(Output::Exit(status.code().unwrap_or(1) as u32))
                }
                Err(_) => output_tx.blocking_send(Output::Error(
                    "Could not wait for the Application terminal.".into(),
                )),
            };
        });
        Ok(Session::from_parts(
            input,
            output,
            Box::new(Killer(cleanup)),
        ))
    }
}

#[cfg(target_os = "linux")]
pub use linux::open;
