# Supervising self-host on macOS

Decision: use launchd directly for the Platform in the MVP. Use s6 for future
Applications without containers, as recorded in [TODO.md](../../TODO.md).
This comparison is documentary research; neither option was tested on the Mac.

Configured as a LaunchDaemon, launchd can start a system service before login and
keep it running. LaunchAgents are tied to a user's login session.
[Apple's launchd guide](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html).

## Options

| Concern | launchd directly | launchd with s6 |
| --- | --- | --- |
| Process tree | macOS -> launchd -> self-host | macOS -> launchd -> s6 -> self-host |
| Start before login | LaunchDaemon | LaunchDaemon starts the s6 supervision tree |
| Restart after exit | KeepAlive policy | s6-supervise restarts the process |
| Preparation | Platform service configuration | Service configuration, s6 installation and supervision directories |
| File logs | Output redirection; rotation needs additional configuration | s6-log supports configured rotation and retention |
| Future native Applications | A launchd job per process | s6 service definitions, potentially reusable across systems |

The s6 alternative uses
[s6-svscan](https://skarnet.org/software/s6/s6-svscan.html) to maintain supervisors,
[s6-supervise](https://skarnet.org/software/s6/s6-supervise.html) for each process
and [s6-log](https://skarnet.org/software/s6/s6-log.html) for log handling.

For the current Mac-only Platform with Compose Applications, direct launchd
supervision requires fewer components. s6 remains the chosen future direction
for Applications without containers. Reusing s6 definitions on another operating
system would still require integration with that system's startup mechanism.

## Limits shared by both options

The Docker runtime must independently support startup without login. Choosing the
Platform's supervisor does not supply that integration or guarantee access to
storage and credentials.

Restarting an exited process does not detect every stalled Application or ensure
its dependencies are ready. s6 supports readiness notification, and
[s6-notifyoncheck](https://skarnet.org/software/s6/s6-notifyoncheck.html) can probe
until startup readiness; that is not continuous Application health monitoring.

FileVault disk unlock is separate from starting a daemon. Apple's documented
remote unlock over SSH on supported Apple Silicon Macs with macOS 26 or later
still requires intervention and does not itself meet unattended startup.
[FileVault encryption](https://support.apple.com/guide/security/volume-encryption-with-filevault-sec4c6dc1b6e/web),
[remote unlock](https://support.apple.com/en-au/guide/security/sec8447f5049/web).

FileVault management is not an MVP feature. The implementation must document and
test the Host conditions under which the required unattended behavior works.
Physical power recovery and sleep behavior are separate from service startup.

See [ADR-0013](../adr/0013-macos-bootstrap-with-launchd.md) for the accepted
Platform supervision decision.
