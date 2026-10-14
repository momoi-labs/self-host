//! `self-host uninstall` (#200): removes what the Platform created on this
//! Host, so the next `init` starts from nothing. It undoes the Platform's own
//! work: Applications and their containers, volumes, networks and images,
//! Virtual machines, native Applications, the data directory, the CA in the
//! system trust store and the host DNS routing. What an installer set up
//! around it, such as launchd daemons or the binary itself, is the
//! installer's to undo (`uninstall.sh` on macOS).
//!
//! Docker may be shared with another Platform, so only names this one owns
//! go: `sf-app-<id>` for its Applications, its custom images and the images
//! its Applications ran.

use crate::apps::{self, ApplicationRecord};
use crate::collection;
use crate::environments::EnvironmentRecord;
use crate::file_store::FileStateStore;
use crate::store::StateStore;
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The name `init` gives the Platform CA, in every trust store.
const CA_NAME: &str = "Self-Host LAN CA";

/// A command run as root: directly when this process already is, through
/// sudo otherwise, so an Operator can run the uninstall as themselves.
fn privileged(program: &str) -> Command {
    // SAFETY: geteuid has no arguments or memory effects.
    if unsafe { libc::geteuid() } == 0 {
        Command::new(program)
    } else {
        let mut command = Command::new("sudo");
        command.arg(program);
        command
    }
}

fn checked(command: &mut Command, step: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("{step}: could not start"))?;
    anyhow::ensure!(status.success(), "{step} failed ({status})");
    Ok(())
}

/// Every Application id this Platform owns: its Applications, the ones
/// removed without their data that still have deployment history, and the
/// ones that left a project directory behind.
pub async fn application_ids(store: Option<&FileStateStore>) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    if let Some(store) = store {
        for app in store.list_applications().await? {
            ids.insert(app.id);
        }
        for deployment in crate::deployments::all(store).await? {
            ids.insert(deployment.application_id);
        }
    }
    if let Some(projects) = apps::project_dir_for("_").parent()
        && let Ok(entries) = std::fs::read_dir(projects)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if apps::is_app_id(&name) {
                ids.insert(name);
            }
        }
    }
    Ok(ids)
}

/// Names matching any of `filters`, from a `docker ... ls`-style command.
/// No filter would list every container on the Host, so none lists nothing.
fn listed(args: &[&str], filters: &[String]) -> Result<Vec<String>> {
    if filters.is_empty() {
        return Ok(Vec::new());
    }
    let mut command = Command::new("docker");
    command.args(args);
    // Repeated name filters are an OR.
    for filter in filters {
        command.args(["--filter", &format!("name={filter}")]);
    }
    let output = command.output()?;
    // A Docker that does not answer lists nothing, which must not read as
    // nothing to remove.
    anyhow::ensure!(
        output.status.success(),
        "docker {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let mut found: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    found.sort();
    found.dedup();
    Ok(found)
}

/// A Docker command whose refusal is a failure: what it would remove stays.
fn docker_checked(command: &mut Command) -> Result<()> {
    let output = command.output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Removes the containers, volumes and networks of these Applications. With
/// `legacy`, a Platform still in an older format that names its Applications
/// nowhere readable, it removes everything with the Platform's prefixes, as
/// reset always did.
pub fn remove_docker_resources(ids: &BTreeSet<String>, legacy: bool) -> Result<()> {
    let owned = |patterns: &[&str], everything: &[&str]| -> Vec<String> {
        if legacy {
            return everything
                .iter()
                .map(|pattern| pattern.to_string())
                .collect();
        }
        ids.iter()
            .flat_map(|id| {
                patterns
                    .iter()
                    .map(move |pattern| pattern.replace("{id}", id))
            })
            // The Platform Infra names are fixed, so they are only this
            // Platform's when it uses the default places.
            .chain(
                ["^self-host", "^sf-system"]
                    .into_iter()
                    .filter(|_| !crate::paths::data_dir_given())
                    .map(str::to_string),
            )
            .collect()
    };

    println!("Stopping and removing containers...");
    // An image Application's one container is sf-app-<id>; a Compose
    // service's is sf-app-<id>-<service>.
    let container_ids = listed(
        &["ps", "-aq"],
        &owned(&["^sf-app-{id}$", "^sf-app-{id}-"], &["self-host", "sf-"]),
    )?;
    if !container_ids.is_empty() {
        docker_checked(
            Command::new("docker")
                .args(["rm", "-f"])
                .args(&container_ids),
        )?;
        println!("Containers removed.");
    } else {
        println!("No containers found.");
    }

    // A Compose Application's named volumes are created under its project,
    // so they are sf-app-<id>_<volume>.
    println!("Removing volumes...");
    let volume_names = listed(
        &["volume", "ls", "-q"],
        &owned(&["^sf-app-{id}_"], &["self-host", "^sf-app-"]),
    )?;
    if !volume_names.is_empty() {
        docker_checked(
            Command::new("docker")
                .args(["volume", "rm"])
                .args(&volume_names),
        )?;
        println!("Volumes removed.");
    } else {
        println!("No volumes found.");
    }

    println!("Removing networks...");
    // Compose names its default network sf-app-<id>_default, and a private
    // provider has sf-private-<id>.
    let mut network_names = listed(
        &["network", "ls", "--format", "{{.Name}}"],
        &owned(
            &["^sf-app-{id}$", "^sf-app-{id}_", "^sf-private-{id}$"],
            &["sf-"],
        ),
    )?;
    network_names.retain(|name| name != "self-host" && name != crate::docker::APP_NETWORK);
    if !network_names.is_empty() {
        let mut remove = Command::new("docker");
        remove.args(["network", "rm"]).args(&network_names);
        // The old prefix match also catches networks this Platform never
        // made, so only its own Applications' networks must go.
        if legacy {
            let _ = remove.output();
        } else {
            docker_checked(&mut remove)?;
        }
    }
    // The shared networks every Platform on this Docker joins. One still in
    // use by another Platform's containers stays: Docker refuses it, and
    // that is no failure.
    let _ = Command::new("docker")
        .args(["network", "rm", "self-host", crate::docker::APP_NETWORK])
        .output();
    println!("Networks removed.");
    Ok(())
}

/// What this Platform has on the Host, read before anything goes.
pub struct Inventory {
    pub data_dir: PathBuf,
    pub ids: BTreeSet<String>,
    pub machines: Vec<EnvironmentRecord>,
    /// Native Applications, which run outside Docker.
    pub native: Vec<ApplicationRecord>,
    /// Images its Applications ran, by reference or immutable id, and its
    /// custom images.
    pub images: BTreeSet<String>,
    /// Custom image ids: every build of one is `sf-img-<id>:<digest>`, and
    /// a rebuild leaves the earlier tags.
    pub custom: BTreeSet<String>,
    pub suffix: Option<String>,
    /// The CA certificate, DER, to find it again in the trust stores.
    pub ca: Option<Vec<u8>>,
    /// macOS resolver files that route a DNS Suffix to this Host.
    pub resolvers: Vec<PathBuf>,
    /// The state, open as the writer, so no daemon starts while this runs.
    _store: Option<FileStateStore>,
}

pub async fn inventory() -> Result<Inventory> {
    let data_dir = crate::paths::platform_config_dir();
    let state = crate::file_store::state_dir();
    anyhow::ensure!(
        !state.join("platform.json").exists(),
        "this Platform still keeps its state in JSON files; import it first \
         (docs/operating.md), or run 'self-host reset --force'"
    );
    // The writer lock, held until the uninstall ends: a daemon still running
    // would go on writing state and starting work while it goes.
    let store = if state.join(crate::file_store::DB_FILE).exists() {
        Some(FileStateStore::open(&state).map_err(|error| {
            match error {
                crate::store::StoreError::Locked(_) => anyhow::anyhow!(
                    "the Platform is running; stop 'self-host serve' first, then uninstall"
                ),
                error => anyhow::Error::new(error)
                    .context("cannot read the Platform state to find what it created"),
            }
        })?)
    } else {
        None
    };

    let ids = application_ids(store.as_ref()).await?;
    let mut machines = Vec::new();
    let mut native = Vec::new();
    let mut images = BTreeSet::new();
    let mut custom = BTreeSet::new();
    if let Some(store) = &store {
        machines = collection::MACHINES.list(store).await?;
        for app in store.list_applications().await? {
            images.extend(apps::images_of(&app));
            if app.source == apps::SOURCE_NATIVE {
                native.push(app);
            }
        }
        for deployment in crate::deployments::all(store).await? {
            images.extend(deployment.images.into_values());
        }
        for image in collection::IMAGES.list(store).await? {
            images.insert(image.image);
            custom.insert(image.id);
        }
    }
    // Native Applications run no image.
    images.retain(|image| !image.is_empty());

    let suffix = std::fs::read(data_dir.join("dns.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|config| config["dns_suffix"].as_str().map(str::to_owned));
    let ca = std::fs::read(data_dir.join("certs").join("ca.pem"))
        .ok()
        .and_then(|bytes| pem::parse(bytes).ok())
        .map(|pem| pem.contents().to_vec());
    let resolvers = resolver_files(Path::new("/etc/resolver"), suffix.as_deref());

    Ok(Inventory {
        data_dir,
        ids,
        machines,
        native,
        images,
        custom,
        suffix,
        ca,
        resolvers,
        _store: store,
    })
}

/// The resolver file for this Platform's DNS Suffix, when self-host wrote it,
/// which it says, or it sends the Suffix to this Host. Tailscale's, which
/// point at 100.100.100.100, are never among them.
fn resolver_files(dir: &Path, suffix: Option<&str>) -> Vec<PathBuf> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let mut local: BTreeSet<String> = crate::host_addresses::interfaces()
        .unwrap_or_default()
        .into_iter()
        .map(|interface| interface.address.to_string())
        .collect();
    local.insert("127.0.0.1".into());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let Ok(text) = std::fs::read_to_string(path) else {
                return false;
            };
            ours(
                &text,
                path.file_name().and_then(|name| name.to_str()),
                suffix,
                &local,
            )
        })
        .collect();
    found.sort();
    found
}

fn ours(text: &str, name: Option<&str>, suffix: Option<&str>, local: &BTreeSet<String>) -> bool {
    // Another Platform on this Mac marks its file the same way, so the name
    // is what ties a file to this Platform's DNS Suffix.
    if name.is_none() || name != suffix {
        return false;
    }
    let marked = text.lines().any(|line| {
        line.trim_start().starts_with('#') && line.to_ascii_lowercase().contains("self-host")
    });
    let here = text.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("nameserver")
            && words.next().is_some_and(|server| local.contains(server))
    });
    marked || here
}

/// A directory only this process's user can enter, for files a root step
/// reads: a fixed name under /tmp could be a symlink someone else planted.
#[cfg(target_os = "linux")]
fn private_dir() -> Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = std::env::temp_dir().join(format!(
        "self-host-uninstall-{:016x}",
        rand::random::<u64>()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// What `run` would remove, one line each, for the Operator to confirm.
pub fn describe(inventory: &Inventory) -> Vec<String> {
    let mut lines = vec![format!(
        "{} Application(s): their containers, volumes, networks and project data",
        inventory.ids.len()
    )];
    if !inventory.machines.is_empty() {
        lines.push(format!(
            "{} Virtual machine(s) and their disks",
            inventory.machines.len()
        ));
    }
    if !inventory.native.is_empty() {
        lines.push(format!(
            "{} native Application(s), their accounts and their data",
            inventory.native.len()
        ));
    }
    lines.push(format!(
        "the images they ran or the Platform built ({}), unless something else uses them",
        inventory.images.len()
    ));
    if inventory.ca.is_some() {
        lines.push(format!(
            "the Platform CA from this Host's trust store ({CA_NAME})"
        ));
    }
    for resolver in &inventory.resolvers {
        lines.push(format!("{}", resolver.display()));
    }
    if cfg!(target_os = "linux") {
        lines.push("self-host-dns.service, if it routes this Platform's DNS Suffix".into());
        if !crate::paths::data_dir_given() {
            lines.push(
                "self-host-native.service, /var/lib/self-host and the bundled s6 in /usr/libexec/self-host"
                    .into(),
            );
        }
    }
    lines.push(format!(
        "{}, with the state, the CA and the API key",
        inventory.data_dir.display()
    ));
    lines
}

/// Removes everything `inventory` found. Each step that fails is reported and
/// the rest still run; the data directory goes only when all of them worked,
/// since it is how a second run knows what is left.
pub async fn run(inventory: &Inventory) -> Vec<String> {
    let mut failures = Vec::new();
    let mut step = |label: &str, result: Result<()>| {
        if let Err(error) = result {
            eprintln!("{label}: {error:#}");
            failures.push(format!("{label}: {error:#}"));
        }
    };

    step(
        "Virtual machines",
        remove_machines(&inventory.machines).await,
    );
    step(
        "native Applications",
        remove_native(&inventory.native).await,
    );
    // A Host that runs only native Applications may have no Docker at all.
    // One whose Docker does not answer still fails below: its containers
    // would stay.
    let docker_installed = !matches!(
        Command::new("docker").arg("--version").output(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    );
    if docker_installed {
        step(
            "containers, volumes and networks",
            remove_docker_resources(&inventory.ids, false),
        );
        step("images", remove_images(inventory).await);
    } else {
        println!("No Docker on this Host; skipping containers, volumes, networks and images.");
    }
    if let Some(ca) = &inventory.ca {
        step("the CA in the trust store", untrust_ca(ca));
    }
    step(
        "host DNS",
        remove_host_dns(&inventory.resolvers, inventory.suffix.as_deref()),
    );

    if failures.is_empty()
        && let Err(error) = remove_native_shared()
    {
        eprintln!("native supervision: {error:#}");
        failures.push(format!("native supervision: {error:#}"));
    }
    if failures.is_empty() && inventory.data_dir.exists() {
        println!("Removing {}...", inventory.data_dir.display());
        if let Err(error) = std::fs::remove_dir_all(&inventory.data_dir) {
            failures.push(format!("{}: {error}", inventory.data_dir.display()));
        }
    }
    failures
}

async fn remove_machines(machines: &[EnvironmentRecord]) -> Result<()> {
    use crate::environments::VmRuntime;
    if machines.is_empty() {
        return Ok(());
    }
    let lima = crate::vms::LimaRuntime::default();
    for machine in machines {
        println!("Deleting Virtual machine {}...", machine.config.name);
        lima.execute(&machine.id, "delete", &machine.config)
            .await
            .map_err(|error| anyhow::anyhow!("{}: {error}", machine.config.name))?;
    }
    Ok(())
}

/// Each native Application goes through the runtime that runs it, with its
/// data: the Platform itself on Linux, as root, and the root helper on macOS.
async fn remove_native(native: &[ApplicationRecord]) -> Result<()> {
    use crate::native::lifecycle::{HostRuntime, NativeRuntime};
    if !native.is_empty() {
        #[cfg(target_os = "linux")]
        anyhow::ensure!(
            // SAFETY: geteuid has no arguments or memory effects.
            unsafe { libc::geteuid() } == 0,
            "native Applications run as their own accounts; run the uninstall as root"
        );
        // A run that got as far as removing the shared supervision tree
        // removed every native Application before it; a retry finds them
        // still in the state, with nothing left to remove.
        #[cfg(target_os = "linux")]
        if !Path::new("/var/lib/self-host/native").exists() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        if !Path::new(crate::native::macos::HELPER).exists() {
            return Ok(());
        }
        let runtime = HostRuntime::default();
        for app in native {
            println!("Removing native Application {}...", app.name);
            runtime.remove(app, true).await?;
        }
    }
    Ok(())
}

/// What every Platform on the Host shares for native Applications. It goes
/// last, once every other step worked: a retry still needs the supervision
/// tree to remove what a failed run left.
fn remove_native_shared() -> Result<()> {
    if crate::paths::data_dir_given() {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        if Path::new("/etc/systemd/system/self-host-native.service").exists() {
            println!("Removing self-host-native.service...");
            checked(
                privileged("systemctl").args(["disable", "--now", "self-host-native.service"]),
                "stopping self-host-native.service",
            )?;
            checked(
                privileged("rm").args(["-f", "/etc/systemd/system/self-host-native.service"]),
                "removing self-host-native.service",
            )?;
            let _ = privileged("systemctl").arg("daemon-reload").status();
        }
        for dir in ["/var/lib/self-host", "/usr/libexec/self-host"] {
            if Path::new(dir).exists() {
                checked(
                    privileged("rm").args(["-rf", dir]),
                    &format!("removing {dir}"),
                )?;
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let helper = Path::new(crate::native::macos::HELPER);
        if helper.exists() {
            println!("Removing native Application accounts...");
            checked(
                privileged(crate::native::macos::HELPER).arg("native-uninstall"),
                "removing native Application accounts (macOS asks for Full Disk Access)",
            )?;
        }
        // The accounts are gone, so macOS lets their homes go too.
        let data = Path::new(crate::native::macos::DATA);
        if data.exists() {
            checked(
                privileged("rm").arg("-rf").arg(data),
                "removing native Application data",
            )?;
        }
    }
    Ok(())
}

/// The images this Platform built or its Applications ran. `docker image rm`
/// without force leaves any another container still uses.
async fn remove_images(inventory: &Inventory) -> Result<()> {
    use crate::docker::{CliDocker, DockerRuntime};
    let docker = CliDocker::new();
    let references = docker.image_references().await?;
    // This Platform's containers are gone by now, so any container left is
    // someone else's. Whatever it runs stays, under every tag: removing one
    // tag of an image another Platform uses would take its reference. Its
    // immutable id is what says which image that is.
    let in_use = container_image_ids()?;
    let mut doomed = BTreeSet::new();
    for (reference, id, _) in references {
        if in_use.contains(&id) {
            continue;
        }
        let owner = |prefix: &str| {
            reference
                .strip_prefix(prefix)
                .and_then(|rest| rest.get(..12))
                .map(str::to_owned)
        };
        let built = owner("sf-source-").is_some_and(|owner| inventory.ids.contains(&owner))
            || owner("sf-img-").is_some_and(|owner| inventory.custom.contains(&owner));
        if built || inventory.images.contains(&reference) || inventory.images.contains(&id) {
            doomed.insert(reference);
        }
    }
    if doomed.is_empty() {
        return Ok(());
    }
    println!("Removing {} image(s)...", doomed.len());
    let mut kept = Vec::new();
    for reference in doomed {
        if !docker.remove_image(&reference).await.unwrap_or(false) {
            kept.push(reference);
        }
    }
    anyhow::ensure!(kept.is_empty(), "Docker kept {}", kept.join(", "));
    Ok(())
}

/// The immutable image id of every container on the Host.
fn container_image_ids() -> Result<BTreeSet<String>> {
    let containers = Command::new("docker").args(["ps", "-aq"]).output()?;
    anyhow::ensure!(containers.status.success(), "docker ps failed");
    let ids: Vec<String> = String::from_utf8_lossy(&containers.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let inspected = Command::new("docker")
        .args(["inspect", "--format", "{{.Image}}"])
        .args(&ids)
        .output()?;
    anyhow::ensure!(inspected.status.success(), "docker inspect failed");
    Ok(String::from_utf8_lossy(&inspected.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect())
}

fn untrust_ca(der: &[u8]) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, der);
        let sha1: String = digest
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect();
        let listed = Command::new("security")
            .args(["find-certificate", "-a", "-Z", "-c", CA_NAME])
            .arg("/Library/Keychains/System.keychain")
            .output()?;
        // Only this Platform's CA: another install minted its own.
        if String::from_utf8_lossy(&listed.stdout).contains(&sha1) {
            println!("Removing the Platform CA from the System Keychain...");
            checked(
                privileged("security")
                    .args(["delete-certificate", "-Z", &sha1])
                    .arg("/Library/Keychains/System.keychain"),
                "removing the CA from the System Keychain",
            )?;
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Removing the anchor and regenerating the bundle are two steps. A
        // marker in the data directory, which a failed run keeps, makes the
        // next run regenerate even though the anchor is already gone.
        let pending = crate::paths::platform_config_dir().join(".trust-refresh-pending");
        let anchor = Path::new("/usr/local/share/ca-certificates/self-host-ca.crt");
        let ours = std::fs::read(anchor)
            .ok()
            .and_then(|bytes| pem::parse(bytes).ok())
            .is_some_and(|pem| pem.contents() == der);
        if ours {
            println!("Removing the Platform CA from the system trust store...");
            std::fs::write(&pending, b"")?;
            checked(privileged("rm").arg("-f").arg(anchor), "removing the CA")?;
        }
        if pending.exists() {
            checked(
                &mut privileged("update-ca-certificates"),
                "updating the system trust store",
            )?;
            std::fs::remove_file(&pending)?;
        }

        // p11-kit keeps anchors in its own format; ask it instead of
        // guessing at files, and only ask for root when the CA is there.
        // Every install mints its own CA under the same name, so only the
        // certificate itself says whether this one is anchored.
        if Path::new("/etc/arch-release").exists() {
            let scratch = private_dir()?;
            let bundle = scratch.join("anchors.pem");
            let _ = Command::new("trust")
                .args(["extract", "--format=pem-bundle", "--filter=ca-anchors"])
                .arg(&bundle)
                .output();
            let anchored = std::fs::read(&bundle)
                .ok()
                .and_then(|bytes| pem::parse_many(bytes).ok())
                .is_some_and(|anchors| anchors.iter().any(|anchor| anchor.contents() == der));
            if anchored {
                println!("Removing the Platform CA from the p11-kit trust store...");
                let file = scratch.join("ca.pem");
                std::fs::write(
                    &file,
                    pem::encode(&pem::Pem::new("CERTIFICATE", der.to_vec())),
                )?;
                let result = checked(
                    privileged("trust").args(["anchor", "--remove"]).arg(&file),
                    "removing the CA from p11-kit",
                );
                let _ = std::fs::remove_dir_all(&scratch);
                result?;
            } else {
                let _ = std::fs::remove_dir_all(&scratch);
            }
        }

        // The browsers' own store: the same nickname may be another
        // Platform's CA, so the certificate is compared first.
        if let Some(home) = dirs::home_dir() {
            let nss = home.join(".pki/nssdb");
            if nss.join("cert9.db").exists() {
                let database = format!("sql:{}", nss.display());
                let listed = Command::new("certutil")
                    .args(["-L", "-a", "-n", CA_NAME, "-d", &database])
                    .output();
                let matches = listed
                    .ok()
                    .and_then(|output| pem::parse_many(output.stdout).ok())
                    .is_some_and(|certs| certs.iter().any(|cert| cert.contents() == der));
                if matches {
                    checked(
                        Command::new("certutil").args(["-D", "-n", CA_NAME, "-d", &database]),
                        "removing the CA from the browsers' certificate database",
                    )?;
                }
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = der;
    Ok(())
}

fn remove_host_dns(resolvers: &[PathBuf], suffix: Option<&str>) -> Result<()> {
    for resolver in resolvers {
        println!("Removing {}...", resolver.display());
        checked(
            privileged("rm").arg("-f").arg(resolver),
            "removing a resolver file",
        )?;
    }
    // One unit routes one DNS Suffix; it is this Platform's when it routes
    // this Platform's.
    #[cfg(target_os = "linux")]
    if let Some(suffix) = suffix
        && std::fs::read_to_string("/etc/systemd/system/self-host-dns.service")
            .is_ok_and(|unit| unit.contains(&format!(" ~{suffix}\n")))
    {
        println!("Removing self-host-dns.service...");
        // Its ExecStop deletes the dummy link that carries the routing.
        checked(
            privileged("systemctl").args(["disable", "--now", "self-host-dns.service"]),
            "stopping self-host-dns.service",
        )?;
        checked(
            privileged("rm").args(["-f", "/etc/systemd/system/self-host-dns.service"]),
            "removing self-host-dns.service",
        )?;
        let _ = privileged("systemctl").arg("daemon-reload").status();
    }
    #[cfg(not(target_os = "linux"))]
    let _ = suffix;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resolver_file_is_ours_when_it_routes_this_platforms_suffix() {
        let local: BTreeSet<String> = ["127.0.0.1".to_string(), "192.168.1.54".to_string()]
            .into_iter()
            .collect();
        let marked = "# Managed by self-host; re-run 'self-host setup-dns' to change it.\nnameserver 127.0.0.1\n";
        assert!(ours(marked, Some("home.lan"), Some("home.lan"), &local));
        // Another Platform's, marked the same way, for its own Suffix.
        assert!(!ours(marked, Some("other.lan"), Some("home.lan"), &local));
        // The installer's file for the current suffix, before it was marked.
        assert!(ours(
            "nameserver 192.168.1.54\n",
            Some("home.lan"),
            Some("home.lan"),
            &local
        ));
        // An earlier suffix that is not marked is not ours to guess at.
        assert!(!ours(
            "nameserver 192.168.1.54\n",
            Some("old.lan"),
            Some("home.lan"),
            &local
        ));
        // Tailscale's.
        assert!(!ours(
            "nameserver 100.100.100.100\n",
            Some("ts.net"),
            Some("home.lan"),
            &local
        ));
        assert!(!ours(
            "nameserver 100.100.100.100\n",
            Some("home.lan"),
            Some("home.lan"),
            &local
        ));
    }
}
