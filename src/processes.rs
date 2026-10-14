//! The processes an Application runs right now, as `ps` would list them.
//!
//! A native Application's come from its cgroup and `/proc` on Linux, and
//! from `ps` under its s6 runner on macOS; a container's from `docker top`.
//! CPU is what `ps` reports: on Linux CPU time over the process's lifetime,
//! on macOS a decaying recent average. Command lines can carry secrets
//! passed as arguments, so the API redacts Variable values before they
//! leave the Host.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Process {
    pub pid: u32,
    /// The parent's pid. A process whose parent is not in the list is a
    /// root of the tree.
    pub ppid: u32,
    pub user: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
}

/// Replaces every non-empty Variable value in the command lines with the
/// same matcher that redacts native output before it reaches the log.
pub fn redact(processes: &mut [Process], environment: &[(String, String)]) {
    let redactor = crate::native::redaction::Redactor::new(environment);
    for process in processes {
        let redacted = redactor.clone().push(process.command.as_bytes(), true);
        process.command = String::from_utf8_lossy(&redacted).into_owned();
    }
}

/// `ps -o pid,ppid,user,pcpu,rss,args`, as `docker top` also runs it: a
/// header, then five fields and the command line, which keeps its spaces.
pub fn parse_ps(output: &str) -> Vec<Process> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let mut fields = [""; 5];
            for field in &mut fields {
                let end = rest.find(char::is_whitespace)?;
                *field = &rest[..end];
                rest = rest[end..].trim_start();
            }
            Some(Process {
                pid: fields[0].parse().ok()?,
                ppid: fields[1].parse().ok()?,
                user: fields[2].into(),
                cpu_percent: fields[3].parse().ok(),
                rss_bytes: fields[4].parse::<u64>().ok().map(|kib| kib * 1024),
                command: rest.trim_end().into(),
            })
        })
        .collect()
}

/// Everything a macOS native Application runs: the descendants of the root
/// `native-run` that s6 supervises for it. The runner itself is the
/// Platform's, so it is left out and its children are the roots.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn under_runner(processes: Vec<Process>, application_id: &str) -> Vec<Process> {
    let service = format!("/services/{application_id}");
    let Some(runner) = processes
        .iter()
        .find(|p| p.command.contains("native-run --service ") && p.command.ends_with(&service))
        .map(|p| p.pid)
    else {
        return Vec::new();
    };
    let mut parents = vec![runner];
    let mut tree = Vec::new();
    let mut rest = processes;
    while !parents.is_empty() {
        let (children, others): (Vec<_>, Vec<_>) =
            rest.into_iter().partition(|p| parents.contains(&p.ppid));
        rest = others;
        parents = children.iter().map(|p| p.pid).collect();
        tree.extend(children);
    }
    tree
}

/// The fields of `/proc/<pid>/stat` this needs. The command name sits in
/// parentheses and may hold spaces or parentheses itself, so the fields
/// are counted from the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, PartialEq)]
struct Stat {
    name: String,
    ppid: u32,
    /// utime plus stime, in clock ticks.
    cpu_ticks: u64,
    /// Clock ticks after boot.
    start_ticks: u64,
    rss_pages: u64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat(text: &str) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // fields[0] is field 3 of proc(5), the state.
    let field = |number: usize| fields.get(number - 3)?.parse::<u64>().ok();
    Some(Stat {
        name: text[open + 1..close].into(),
        ppid: field(4)? as u32,
        cpu_ticks: field(14)? + field(15)?,
        start_ticks: field(22)?,
        rss_pages: field(24)?,
    })
}

/// The real uid on the `Uid:` line of `/proc/<pid>/status`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// `/proc/<pid>/cmdline` is NUL separated. A zombie has none, and `ps`
/// shows its name in brackets.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn command_line(cmdline: &[u8], name: &str) -> String {
    let args: Vec<String> = cmdline
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    if args.is_empty() {
        format!("[{name}]")
    } else {
        args.join(" ")
    }
}

/// Reads each pid from `/proc`. A process that exits between the cgroup
/// listing and here is left out. `users` names the uids it knows; any
/// other shows as its number.
#[cfg(target_os = "linux")]
pub fn read_proc(pids: &[u32], users: &[(u32, &str)]) -> Vec<Process> {
    // SAFETY: sysconf only reads configuration values.
    let (ticks, page) = unsafe {
        (
            libc::sysconf(libc::_SC_CLK_TCK).max(1) as f64,
            libc::sysconf(libc::_SC_PAGESIZE).max(0) as u64,
        )
    };
    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok());
    pids.iter()
        .filter_map(|&pid| {
            let proc = std::path::PathBuf::from(format!("/proc/{pid}"));
            let stat = parse_stat(&std::fs::read_to_string(proc.join("stat")).ok()?)?;
            let uid = parse_uid(&std::fs::read_to_string(proc.join("status")).ok()?)?;
            let cmdline = std::fs::read(proc.join("cmdline")).ok()?;
            let elapsed = uptime.map(|uptime| uptime - stat.start_ticks as f64 / ticks);
            Some(Process {
                pid,
                ppid: stat.ppid,
                user: users
                    .iter()
                    .find(|(known, _)| *known == uid)
                    .map_or_else(|| uid.to_string(), |(_, name)| (*name).into()),
                command: command_line(&cmdline, &stat.name),
                cpu_percent: elapsed
                    .filter(|elapsed| *elapsed > 0.0)
                    .map(|elapsed| stat.cpu_ticks as f64 / ticks / elapsed * 100.0),
                rss_bytes: Some(stat.rss_pages * page),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_top_keeps_the_spaces_of_a_command_line() {
        let output = "PID    PPID   USER   %CPU  RSS   COMMAND\n\
                      4101   4080   root   0.5   2048  /bin/sh -c node server.js\n\
                      4120   4101   1000   12.0  81920 node server.js --port 3000\n";
        assert_eq!(
            parse_ps(output),
            vec![
                Process {
                    pid: 4101,
                    ppid: 4080,
                    user: "root".into(),
                    command: "/bin/sh -c node server.js".into(),
                    cpu_percent: Some(0.5),
                    rss_bytes: Some(2048 * 1024),
                },
                Process {
                    pid: 4120,
                    ppid: 4101,
                    user: "1000".into(),
                    command: "node server.js --port 3000".into(),
                    cpu_percent: Some(12.0),
                    rss_bytes: Some(81920 * 1024),
                },
            ]
        );
    }

    #[test]
    fn a_macos_tree_is_what_the_runner_started_and_nothing_else() {
        let output = "  PID  PPID USER %CPU RSS ARGS\n\
            12700 12689 root 0.0 1472 s6-supervise ymau548satec\n\
            12707 12700 root 0.0 900 /Library/PrivilegedHelperTools/dev.momoi.self-host native-run --service /Library/Application Support/self-host/native/services/ymau548satec\n\
            12711 12707 sf-app-ymau548satec 1.5 554464 python laya-apple serve\n\
            12715 12711 sf-app-ymau548satec 0.2 887728 python -m laya_apple.executor\n\
            12705 12702 root 0.0 900 /Library/PrivilegedHelperTools/dev.momoi.self-host native-run --service /Library/Application Support/self-host/native/services/d8nmhnsy3wvn\n\
            12712 12705 sf-app-d8nmhnsy3wvn 0.0 290224 jellyfin --datadir data\n\
            25122 1 sf-app-ymau548satec 0.0 2640 /usr/sbin/distnoted agent\n";
        let tree = under_runner(parse_ps(output), "ymau548satec");
        assert_eq!(
            tree.iter().map(|p| (p.pid, p.ppid)).collect::<Vec<_>>(),
            [(12711, 12707), (12715, 12711)]
        );
        assert!(under_runner(parse_ps(output), "stopped00000").is_empty());
    }

    #[test]
    fn stat_is_read_past_a_name_with_spaces_and_parentheses() {
        let text = "812 (my (odd) app) S 800 812 800 0 -1 4194560 100 0 0 0 \
                    250 50 0 0 20 0 1 0 9000 1000000 300 18446744073709551615";
        assert_eq!(
            parse_stat(text),
            Some(Stat {
                name: "my (odd) app".into(),
                ppid: 800,
                cpu_ticks: 300,
                start_ticks: 9000,
                rss_pages: 300,
            })
        );
    }

    #[test]
    fn uid_is_the_real_one_and_a_zombie_shows_its_name() {
        assert_eq!(
            parse_uid("Name:\tnode\nUid:\t1001\t1001\t1001\t1001\n"),
            Some(1001)
        );
        assert_eq!(command_line(b"node\0server.js\0", "node"), "node server.js");
        assert_eq!(command_line(b"", "node"), "[node]");
    }

    #[test]
    fn variable_values_are_redacted_longest_first() {
        let mut processes = vec![Process {
            pid: 1,
            ppid: 0,
            user: "app".into(),
            command: "worker --auth token-long --key token --empty".into(),
            cpu_percent: None,
            rss_bytes: None,
        }];
        redact(
            &mut processes,
            &[
                ("A".into(), "token".into()),
                ("B".into(), "token-long".into()),
                ("EMPTY".into(), "".into()),
            ],
        );
        assert_eq!(
            processes[0].command,
            "worker --auth [redacted] --key [redacted] --empty"
        );
    }
}
