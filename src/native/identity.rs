//! Application Accounts: the execution identity of a native Application.
//!
//! Each Application gets a dedicated non-login account. Linux uses useradd;
//! macOS uses the local directory through the restricted helper. Both reject
//! root, the Platform identity, and accounts without our ownership marker.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
#[path = "identity_macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{provision, purge, retired, revoke};

/// The marker `provision` writes into the account's comment field, followed
/// by the Application id. An existing account without it is not ours.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
const COMMENT_MARKER: &str = "self-host Application ";

/// The marker `revoke` leaves on a macOS account it retired, followed by the
/// Application id. The record stays, so its numbers are never handed out
/// again, and nothing adopts it as a live account.
#[cfg(target_os = "macos")]
const RETIRED_MARKER: &str = "self-host retired Application ";

/// The prefix every Application Account name carries, the same one the
/// container Runtime uses for project and container names.
const ACCOUNT_PREFIX: &str = "sf-app-";

/// Where an Application Account's shell points: a program that refuses to
/// start a session, so the account is an identity and not a login.
#[cfg(target_os = "linux")]
const NOLOGIN: &str = "/usr/sbin/nologin";

/// A validated account name: 1 to 32 characters, `^[a-z_][a-z0-9_-]*$`, and
/// never `root`. Nothing is trimmed; a name with whitespace is invalid.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountName(String);

impl AccountName {
    pub fn parse(name: &str) -> Result<Self, IdentityError> {
        if name.is_empty() {
            return Err(IdentityError::Empty);
        }
        if name.len() > 32 {
            return Err(IdentityError::Invalid(name.to_string()));
        }
        let mut chars = name.chars();
        let first = chars.next().expect("a non-empty name has a first char");
        if !(first.is_ascii_lowercase() || first == '_') {
            return Err(IdentityError::Invalid(name.to_string()));
        }
        if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return Err(IdentityError::Invalid(name.to_string()));
        }
        if name == "root" {
            return Err(IdentityError::Root(name.to_string()));
        }
        Ok(AccountName(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The conventional name for an Application's account: `sf-app-<id>`.
pub fn account_name_for(application_id: &str) -> Result<AccountName, IdentityError> {
    AccountName::parse(&format!("{ACCOUNT_PREFIX}{application_id}"))
}

/// The comment `provision` records on the account it creates.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
fn comment_for(application_id: &str) -> String {
    format!("{COMMENT_MARKER}{application_id}")
}

/// The comment a retired macOS account carries.
#[cfg(target_os = "macos")]
fn retired_comment_for(application_id: &str) -> String {
    format!("{RETIRED_MARKER}{application_id}")
}

/// Whether an existing account's comment says the Platform created it for
/// this Application. Anything else, including another Application's marker,
/// is an account we must not touch.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
fn comment_marks_ours(comment: &str, application_id: &str) -> bool {
    comment == comment_for(application_id)
}

/// An Application Account as the Host knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAccount {
    pub name: AccountName,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

#[derive(Debug)]
pub enum IdentityError {
    Empty,
    Invalid(String),
    /// No account by that name on the Host.
    Unknown(String),
    /// uid 0 or gid 0, or the literal name `root`.
    Root(String),
    /// The account is the one the Platform itself runs as.
    Platform(String),
    Lookup(std::io::Error),
    /// Not Linux.
    Unsupported,
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::Empty => write!(f, "the Application Account name is empty"),
            IdentityError::Invalid(name) => write!(
                f,
                "'{name}' is not a valid Application Account name: 1 to 32 characters, \
                 lowercase letters, digits, '_' and '-', starting with a letter or '_'"
            ),
            IdentityError::Unknown(name) => {
                write!(
                    f,
                    "the Application Account '{name}' does not exist on the Host"
                )
            }
            IdentityError::Root(name) => write!(
                f,
                "the Application Account '{name}' is root; the Platform never runs an Application as root"
            ),
            IdentityError::Platform(name) => write!(
                f,
                "the Application Account '{name}' is the account the Platform runs as"
            ),
            IdentityError::Lookup(_) => write!(f, "failed to look up the Application Account"),
            IdentityError::Unsupported => write!(f, "native Applications require Linux or macOS"),
        }
    }
}

impl std::error::Error for IdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IdentityError::Lookup(e) => Some(e),
            _ => None,
        }
    }
}

/// Looks the account up in the Host's user database and refuses root and
/// the Platform's own account. The result is what a launch runs as.
pub fn resolve(name: &AccountName) -> Result<ResolvedAccount, IdentityError> {
    let entry = lookup(name)?;
    check_account(name, &entry)?;
    Ok(ResolvedAccount {
        name: name.clone(),
        uid: entry.uid,
        gid: entry.gid,
        home: entry.home,
    })
}

/// The passwd fields the Platform reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PasswdEntry {
    uid: u32,
    gid: u32,
    home: PathBuf,
    comment: String,
}

/// The identities an Application may never run as: root, and whoever the
/// Platform is (real or effective), so a compromised Application does not
/// reach the Platform's own files.
fn check_account(name: &AccountName, entry: &PasswdEntry) -> Result<(), IdentityError> {
    if entry.uid == 0 || entry.gid == 0 {
        return Err(IdentityError::Root(name.as_str().to_string()));
    }
    // SAFETY: getuid and geteuid take no arguments and cannot fail.
    let (real, effective) = unsafe { (libc::getuid(), libc::geteuid()) };
    if entry.uid == real || entry.uid == effective {
        return Err(IdentityError::Platform(name.as_str().to_string()));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn lookup(name: &AccountName) -> Result<PasswdEntry, IdentityError> {
    use std::ffi::{CStr, CString};

    let c_name = CString::new(name.as_str()).map_err(|_| {
        IdentityError::Lookup(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the account name holds a NUL byte",
        ))
    })?;
    let mut buffer: Vec<u8> = vec![0; 16 * 1024];
    loop {
        // SAFETY: passwd is plain data that getpwnam_r fills in completely on
        // success; the zeroed value is never read before that.
        let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call, the buffer length is
        // its real length, and glibc writes the strings it points `passwd` at
        // into that buffer, which outlives the reads below.
        let code = unsafe {
            libc::getpwnam_r(
                c_name.as_ptr(),
                &mut passwd,
                buffer.as_mut_ptr().cast::<libc::c_char>(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if code != 0 {
            return Err(IdentityError::Lookup(std::io::Error::from_raw_os_error(
                code,
            )));
        }
        if result.is_null() {
            return Err(IdentityError::Unknown(name.as_str().to_string()));
        }
        // SAFETY: getpwnam_r succeeded, so pw_dir and pw_gecos point at NUL
        // terminated strings inside `buffer`.
        let (home, comment) = unsafe {
            (
                CStr::from_ptr(passwd.pw_dir).to_string_lossy().into_owned(),
                CStr::from_ptr(passwd.pw_gecos)
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        return Ok(PasswdEntry {
            uid: passwd.pw_uid,
            gid: passwd.pw_gid,
            home: PathBuf::from(home),
            comment,
        });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn lookup(_name: &AccountName) -> Result<PasswdEntry, IdentityError> {
    Err(IdentityError::Unsupported)
}

/// What `provision` needs: the Application the account is for, the name it
/// gets, and where its home goes.
#[derive(Debug, Clone)]
pub struct ProvisionRequest<'a> {
    pub application_id: &'a str,
    pub account: &'a AccountName,
    pub home: &'a Path,
}

/// Verify both the ownership marker and home before lifecycle mutation.
pub fn verify_owned(request: &ProvisionRequest) -> Result<ResolvedAccount, ProvisionError> {
    let entry = lookup(request.account)?;
    if !comment_marks_ours(&entry.comment, request.application_id) || entry.home != request.home {
        return Err(ProvisionError::NotOurs(request.account.as_str().to_owned()));
    }
    check_account(request.account, &entry)?;
    #[cfg(target_os = "macos")]
    macos::verify(request, &entry)?;
    Ok(ResolvedAccount {
        name: request.account.clone(),
        uid: entry.uid,
        gid: entry.gid,
        home: entry.home,
    })
}

/// Remove an owned execution identity, never its retained home or data.
#[cfg(target_os = "linux")]
pub fn revoke(request: &ProvisionRequest) -> Result<(), ProvisionError> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(ProvisionError::NotPrivileged);
    }
    match verify_owned(request) {
        Ok(_) => (),
        Err(ProvisionError::Identity(IdentityError::Unknown(_))) => return Ok(()),
        Err(error) => return Err(error),
    }
    let step = "revoke the Application Account with userdel".to_owned();
    let output = std::process::Command::new("/usr/sbin/userdel")
        .arg(request.account.as_str())
        .env_clear()
        .output()
        .map_err(|source| ProvisionError::Command {
            step: step.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(ProvisionError::Failed {
            step,
            stderr: CommandStderr(String::from_utf8_lossy(&output.stderr).into_owned()),
        });
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn revoke(_: &ProvisionRequest) -> Result<(), ProvisionError> {
    Err(ProvisionError::Unsupported)
}

/// The text a tool wrote on stderr when it refused, kept as its own layer
/// under the step that failed (ADR-0010).
#[derive(Debug)]
pub struct CommandStderr(pub String);

impl std::fmt::Display for CommandStderr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = self.0.trim();
        if text.is_empty() {
            write!(f, "the command exited with an error and said nothing")
        } else {
            f.write_str(text)
        }
    }
}

impl std::error::Error for CommandStderr {}

#[derive(Debug)]
pub enum ProvisionError {
    /// Provisioning runs as root or not at all.
    NotPrivileged,
    Identity(IdentityError),
    /// An account by that name exists and the Platform did not create it.
    NotOurs(String),
    /// A step could not even start.
    Command {
        step: String,
        source: std::io::Error,
    },
    /// A step ran and refused.
    Failed {
        step: String,
        stderr: CommandStderr,
    },
    /// Not Linux.
    Unsupported,
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionError::NotPrivileged => {
                write!(f, "provisioning an Application Account requires root")
            }
            ProvisionError::Identity(_) => write!(f, "the Application Account cannot be used"),
            ProvisionError::NotOurs(name) => write!(
                f,
                "the account '{name}' exists and the Platform did not create it"
            ),
            ProvisionError::Command { step, .. } | ProvisionError::Failed { step, .. } => {
                write!(f, "failed to {step}")
            }
            ProvisionError::Unsupported => write!(f, "native Applications require Linux or macOS"),
        }
    }
}

impl std::error::Error for ProvisionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProvisionError::Identity(e) => Some(e),
            ProvisionError::Command { source, .. } => Some(source),
            ProvisionError::Failed { stderr, .. } => Some(stderr),
            _ => None,
        }
    }
}

impl From<IdentityError> for ProvisionError {
    fn from(e: IdentityError) -> Self {
        ProvisionError::Identity(e)
    }
}

/// Creates the Application Account as root, or accepts the one the Platform
/// created before. The account is a system account with a group of its own,
/// a nologin shell, no supplementary groups, and a home of mode 0750 that it
/// owns. Idempotent: running it twice leaves the same account.
#[cfg(target_os = "linux")]
pub fn provision(request: &ProvisionRequest) -> Result<ResolvedAccount, ProvisionError> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    // SAFETY: geteuid takes no arguments and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return Err(ProvisionError::NotPrivileged);
    }

    let entry = match lookup(request.account) {
        Ok(entry) => {
            if !comment_marks_ours(&entry.comment, request.application_id) {
                return Err(ProvisionError::NotOurs(
                    request.account.as_str().to_string(),
                ));
            }
            entry
        }
        Err(IdentityError::Unknown(_)) => {
            let step = "create the Application Account with useradd".to_string();
            let output = Command::new("/usr/sbin/useradd")
                .env_clear()
                .args([
                    "--system",
                    "--user-group",
                    "--create-home",
                    "--shell",
                    NOLOGIN,
                ])
                .arg("--home-dir")
                .arg(request.home)
                .arg("--comment")
                .arg(comment_for(request.application_id))
                .arg(request.account.as_str())
                .output()
                .map_err(|source| ProvisionError::Command {
                    step: step.clone(),
                    source,
                })?;
            if !output.status.success() {
                return Err(ProvisionError::Failed {
                    step,
                    stderr: CommandStderr(String::from_utf8_lossy(&output.stderr).into_owned()),
                });
            }
            lookup(request.account)?
        }
        Err(e) => return Err(ProvisionError::Identity(e)),
    };
    check_account(request.account, &entry)?;

    // useradd made the home with whatever mode login.defs says; the Platform
    // wants exactly this, whoever created the directory.
    std::os::unix::fs::chown(&entry.home, Some(entry.uid), Some(entry.gid)).map_err(|source| {
        ProvisionError::Command {
            step: "make the Application Account own its home".to_string(),
            source,
        }
    })?;
    std::fs::set_permissions(&entry.home, std::fs::Permissions::from_mode(0o750)).map_err(
        |source| ProvisionError::Command {
            step: "set the mode of the Application Account's home".to_string(),
            source,
        },
    )?;

    Ok(ResolvedAccount {
        name: request.account.clone(),
        uid: entry.uid,
        gid: entry.gid,
        home: entry.home,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn provision(_request: &ProvisionRequest) -> Result<ResolvedAccount, ProvisionError> {
    Err(ProvisionError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_name_parses_and_reads_back_unchanged() {
        let name = AccountName::parse("sf-app-k3n8qz4v2x1p").unwrap();
        assert_eq!(name.as_str(), "sf-app-k3n8qz4v2x1p");
        assert_eq!(name.to_string(), "sf-app-k3n8qz4v2x1p");
    }

    #[test]
    fn an_underscore_may_start_a_name_and_digits_may_follow() {
        assert!(AccountName::parse("_svc").is_ok());
        assert!(AccountName::parse("a1_b-2").is_ok());
        assert!(AccountName::parse("a").is_ok());
    }

    #[test]
    fn an_empty_name_is_empty_and_not_invalid() {
        assert!(matches!(AccountName::parse(""), Err(IdentityError::Empty)));
    }

    #[test]
    fn whitespace_is_never_trimmed_away() {
        for name in [" sf-app-x", "sf-app-x ", "sf app", "sf-app-x\n"] {
            assert!(
                matches!(AccountName::parse(name), Err(IdentityError::Invalid(n)) if n == name),
                "{name:?} should be invalid"
            );
        }
    }

    #[test]
    fn uppercase_a_leading_digit_or_hyphen_and_other_symbols_are_invalid() {
        for name in ["Sf-app", "1abc", "-abc", "a.b", "a:b", "a/b", "sf-app-é"] {
            assert!(
                matches!(AccountName::parse(name), Err(IdentityError::Invalid(_))),
                "{name:?} should be invalid"
            );
        }
    }

    #[test]
    fn thirty_two_characters_fit_and_thirty_three_do_not() {
        let fits = "a".repeat(32);
        let too_long = "a".repeat(33);
        assert!(AccountName::parse(&fits).is_ok());
        assert!(matches!(
            AccountName::parse(&too_long),
            Err(IdentityError::Invalid(_))
        ));
    }

    #[test]
    fn root_is_refused_by_name_before_anything_is_looked_up() {
        assert!(matches!(
            AccountName::parse("root"),
            Err(IdentityError::Root(n)) if n == "root"
        ));
    }

    #[test]
    fn the_conventional_account_name_carries_the_application_prefix() {
        let name = account_name_for("k3n8qz4v2x1p").unwrap();
        assert_eq!(name.as_str(), "sf-app-k3n8qz4v2x1p");
    }

    #[test]
    fn an_application_id_that_breaks_the_name_rules_is_refused() {
        assert!(matches!(
            account_name_for("K3N8"),
            Err(IdentityError::Invalid(_))
        ));
        assert!(matches!(
            account_name_for(&"x".repeat(30)),
            Err(IdentityError::Invalid(_))
        ));
    }

    #[test]
    fn only_the_exact_marker_for_this_application_counts_as_ours() {
        assert!(comment_marks_ours(
            "self-host Application k3n8qz4v2x1p",
            "k3n8qz4v2x1p"
        ));
        assert!(!comment_marks_ours(
            "self-host Application other",
            "k3n8qz4v2x1p"
        ));
        assert!(!comment_marks_ours("", "k3n8qz4v2x1p"));
        assert!(!comment_marks_ours("Operator", "k3n8qz4v2x1p"));
    }

    #[test]
    fn a_uid_or_gid_of_zero_is_root_whatever_the_name_says() {
        let name = AccountName::parse("sf-app-k3n8qz4v2x1p").unwrap();
        let root_uid = PasswdEntry {
            uid: 0,
            gid: 1000,
            home: PathBuf::from("/"),
            comment: String::new(),
        };
        let root_gid = PasswdEntry {
            uid: 1000,
            gid: 0,
            home: PathBuf::from("/"),
            comment: String::new(),
        };
        assert!(matches!(
            check_account(&name, &root_uid),
            Err(IdentityError::Root(_))
        ));
        assert!(matches!(
            check_account(&name, &root_gid),
            Err(IdentityError::Root(_))
        ));
    }

    #[test]
    fn the_platforms_own_uid_is_refused() {
        let name = AccountName::parse("sf-app-k3n8qz4v2x1p").unwrap();
        // SAFETY: getuid takes no arguments and cannot fail.
        let me = unsafe { libc::getuid() };
        let entry = PasswdEntry {
            uid: me,
            gid: 65534,
            home: PathBuf::from("/"),
            comment: String::new(),
        };
        let expected_root = me == 0;
        match check_account(&name, &entry) {
            Err(IdentityError::Root(_)) if expected_root => {}
            Err(IdentityError::Platform(n)) if !expected_root => assert_eq!(n, name.as_str()),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn errors_state_their_own_layer_and_keep_the_cause_underneath() {
        let e = ProvisionError::Failed {
            step: "create the Application Account with useradd".into(),
            stderr: CommandStderr("useradd: cannot lock /etc/passwd\n".into()),
        };
        assert_eq!(
            e.to_string(),
            "failed to create the Application Account with useradd"
        );
        let report = crate::error::ErrorReport::new(&e);
        assert_eq!(report.caused_by, ["useradd: cannot lock /etc/passwd"]);

        let silent = CommandStderr(String::new());
        assert_eq!(
            silent.to_string(),
            "the command exited with an error and said nothing"
        );

        let e = ProvisionError::Identity(IdentityError::Root("root".into()));
        let report = crate::error::ErrorReport::new(&e);
        assert_eq!(report.error, "the Application Account cannot be used");
        assert_eq!(report.caused_by.len(), 1);
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn resolving_and_provisioning_are_unsupported_off_linux() {
        let name = AccountName::parse("sf-app-k3n8qz4v2x1p").unwrap();
        assert!(matches!(resolve(&name), Err(IdentityError::Unsupported)));
        let request = ProvisionRequest {
            application_id: "k3n8qz4v2x1p",
            account: &name,
            home: Path::new("/var/lib/self-host/apps/k3n8qz4v2x1p"),
        };
        assert!(matches!(
            provision(&request),
            Err(ProvisionError::Unsupported)
        ));
    }
}
