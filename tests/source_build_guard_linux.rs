//! Requires root on disposable Linux. Does not run in ordinary Cargo checks.
#![cfg(target_os = "linux")]

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

const UID: u32 = 62423;
const HELPER: &str = r#"
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/prctl.h>
#include <unistd.h>
int main(int argc, char **argv) {
    uid_t real_uid, effective_uid, saved_uid;
    if (getresuid(&real_uid, &effective_uid, &saved_uid) != 0) return 1;
    printf("uids=%u,%u,%u\n", real_uid, effective_uid, saved_uid);
    printf("NoNewPrivs=%d\n", prctl(PR_GET_NO_NEW_PRIVS, 0UL, 0UL, 0UL, 0UL));
    if (argc > 1) {
        int result = setuid(0);
        printf("setuid_result=%d\n", result);
        if (getresuid(&real_uid, &effective_uid, &saved_uid) != 0) return 1;
        printf("after=%u,%u,%u\n", real_uid, effective_uid, saved_uid);
    }
    fflush(stdout);
    return 0;
}
"#;

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn child(path: &std::path::Path) -> Command {
    let mut command = Command::new(path);
    command.env_clear().env("PATH", "/usr/bin:/bin");
    // SAFETY: fixed synthetic ids, no shared Rust state or allocating calls.
    unsafe {
        command.pre_exec(|| {
            if libc::setgroups(0, std::ptr::null()) != 0
                || libc::setresgid(UID, UID, UID) != 0
                || libc::setresuid(UID, UID, UID) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

fn capable_child(path: &std::path::Path) -> Command {
    #[repr(C)]
    struct Header {
        version: u32,
        pid: libc::c_int,
    }
    #[repr(C)]
    struct Capability {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut command = Command::new(path);
    command.env_clear().env("PATH", "/usr/bin:/bin");
    // SAFETY: fixed capability structures and scalar ids; no Rust allocation.
    unsafe {
        command.pre_exec(|| {
            let header = Header {
                version: 0x2008_0522,
                pid: 0,
            };
            let capabilities = [
                Capability {
                    effective: 1 << 7,
                    permitted: 1 << 7,
                    inheritable: 1 << 7,
                },
                Capability {
                    effective: 0,
                    permitted: 0,
                    inheritable: 0,
                },
            ];
            if libc::prctl(libc::PR_SET_KEEPCAPS, 1, 0, 0, 0) != 0
                || libc::setgroups(0, std::ptr::null()) != 0
                || libc::setresgid(UID, UID, UID) != 0
                || libc::setresuid(UID, UID, UID) != 0
                || libc::syscall(libc::SYS_capset, &header, capabilities.as_ptr()) != 0
                || libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_RAISE, 7, 0, 0) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

#[test]
#[ignore = "requires root and a static C compiler on disposable Linux"]
fn trusted_launcher_blocks_setuid_elevation_and_refuses_root() {
    // SAFETY: geteuid cannot fail.
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "run as root on disposable Linux"
    );
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "sf-build-guard-{}-{:032x}",
        std::process::id(),
        rand::random::<u128>()
    )));
    std::fs::create_dir(&fixture.0).unwrap();
    std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    let private = fixture.0.join("private");
    let launcher = self_host::source_build_guard::compile(&private).unwrap();
    assert_eq!(
        std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(launcher.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&launcher).unwrap().permissions().mode() & 0o777,
        0o555
    );
    assert_eq!(std::fs::metadata(&launcher).unwrap().uid(), 0);
    let header = Command::new("/usr/bin/readelf")
        .args(["-l"])
        .arg(&launcher)
        .output()
        .unwrap();
    assert!(header.status.success());
    assert!(
        !String::from_utf8_lossy(&header.stdout).contains("INTERP"),
        "launcher must be static"
    );

    // Copy into a protected executable path to model Docker's root-owned COPY.
    let executable = fixture.0.join("guard");
    std::fs::copy(&launcher, &executable).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o555)).unwrap();
    let helper_source = fixture.0.join("setuid-helper.c");
    let helper = fixture.0.join("setuid-helper");
    std::fs::write(&helper_source, HELPER).unwrap();
    let status = Command::new("/usr/bin/cc")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["-O2", "-static", "-o"])
        .arg(&helper)
        .arg(helper_source)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o4755)).unwrap();
    assert_eq!(std::fs::metadata(&helper).unwrap().uid(), 0);

    let control = child(&helper).output().unwrap();
    assert!(control.status.success());
    let control = String::from_utf8(control.stdout).unwrap();
    assert!(
        control.contains(&format!("uids={UID},0,0")),
        "fixture must prove unguarded elevation: {control}"
    );
    assert!(control.contains("NoNewPrivs=0"));

    let guarded = child(&executable)
        .arg(&helper)
        .env("LD_PRELOAD", "/synthetic/does-not-exist.so")
        .output()
        .unwrap();
    assert!(
        guarded.status.success(),
        "{}",
        String::from_utf8_lossy(&guarded.stderr)
    );
    let guarded = String::from_utf8(guarded.stdout).unwrap();
    assert!(
        guarded.contains(&format!("uids={UID},{UID},{UID}")),
        "guarded descendant gained root: {guarded}"
    );
    assert!(guarded.contains("NoNewPrivs=1"));

    let cap_helper = fixture.0.join("cap-helper");
    std::fs::copy(&helper, &cap_helper).unwrap();
    std::fs::set_permissions(&cap_helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let control_caps = capable_child(&cap_helper).arg("attempt").output().unwrap();
    assert!(control_caps.status.success());
    let control_caps = String::from_utf8(control_caps.stdout).unwrap();
    assert!(control_caps.contains(&format!("uids={UID},{UID},{UID}")));
    assert!(
        control_caps.contains("setuid_result=0"),
        "ambient capability control must prove elevation: {control_caps}"
    );
    assert!(control_caps.contains("after=0,0,0"));
    let guarded_caps = capable_child(&executable)
        .arg(&cap_helper)
        .arg("attempt")
        .output()
        .unwrap();
    assert!(
        guarded_caps.status.success(),
        "{}",
        String::from_utf8_lossy(&guarded_caps.stderr)
    );
    let guarded_caps = String::from_utf8(guarded_caps.stdout).unwrap();
    assert!(guarded_caps.contains("setuid_result=-1"));
    assert!(
        guarded_caps.contains(&format!("after={UID},{UID},{UID}")),
        "capabilities survived guard: {guarded_caps}"
    );
    assert!(guarded_caps.contains("NoNewPrivs=1"));

    let status = child(&executable)
        .args(["/bin/cat", "/proc/self/status"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.lines().any(|line| line == "NoNewPrivs:\t1"));
    let root = Command::new(&executable).arg(&helper).output().unwrap();
    assert_eq!(
        root.status.code(),
        Some(126),
        "launcher must refuse root before executing code"
    );
    assert!(root.stdout.is_empty());
    let mut real_root = Command::new(&executable);
    real_root.arg(&helper);
    // SAFETY: fixed synthetic ids; preserve only real root to test refusal.
    unsafe {
        real_root.pre_exec(|| {
            if libc::setgroups(0, std::ptr::null()) != 0
                || libc::setresgid(UID, UID, UID) != 0
                || libc::setresuid(0, UID, UID) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let real_root = real_root.output().unwrap();
    assert_eq!(
        real_root.status.code(),
        Some(126),
        "real root UID must also be refused"
    );
    assert!(real_root.stdout.is_empty());
    println!(
        "control: uids={UID},0,0; guarded: uids={UID},{UID},{UID}; NoNewPrivs=1; root refused126; static ELF verified"
    );
}
