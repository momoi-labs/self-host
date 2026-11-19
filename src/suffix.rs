use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_json::Value;

use crate::environments::EnvironmentRecord;
use crate::store::{ApplicationRecord, StateStore};

pub struct Plan {
    pub old: String,
    pub new: String,
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
    apps: Vec<ApplicationRecord>,
    machines: Option<Vec<EnvironmentRecord>>,
    cli_url: Option<String>,
}

pub fn rename(name: &str, old: &str, new: &str) -> Option<String> {
    let name = name.to_ascii_lowercase();
    if name == old {
        return Some(new.to_string());
    }
    name.strip_suffix(&format!(".{old}"))
        .map(|label| format!("{label}.{new}"))
}

pub async fn plan<S: StateStore>(store: &S, config_dir: &Path, new: &str) -> anyhow::Result<Plan> {
    let new = new.trim().trim_end_matches('.').to_ascii_lowercase();
    crate::dns::Config::new(&new, Vec::new())?;
    let old = store
        .get_state("dns_suffix")
        .await?
        .context("the Platform is not initialized; run 'self-host init'")?;
    anyhow::ensure!(new != old, "the DNS Suffix is already {old}");
    let dns_path = config_dir.join("dns.json");
    let dns = read_json(&dns_path)?;
    anyhow::ensure!(
        dns["dns_suffix"].as_str() == Some(old.as_str()),
        "{} does not name {old}; run 'self-host init --dns {old}' to repair it first",
        dns_path.display()
    );
    let mut changes = vec![
        format!("state dns_suffix: {old} -> {new}"),
        format!("{}: dns_suffix {old} -> {new}", dns_path.display()),
        format!(
            "{}: LAN certificate *.{old} -> *.{new}, signed by the same CA",
            config_dir.join("certs/cert.pem").display()
        ),
    ];
    let mut warnings = Vec::new();

    let mut apps = Vec::new();
    let mut owners: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let console = format!("console.{new}");
    let legacy = crate::routes::legacy_console_hostname(&console);
    for mut app in store.list_applications().await? {
        let mut lines = Vec::new();
        let before: std::collections::BTreeSet<String> = crate::routes::hostnames(&app)
            .iter()
            .map(|hostname| hostname.to_ascii_lowercase())
            .collect();
        if let Some(name) = rename(&app.hostname, &old, &new) {
            lines.push(format!("hostname {} -> {name}", app.hostname));
            app.hostname = name;
        }
        for alias in &mut app.aliases {
            if let Some(name) = rename(alias, &old, &new) {
                lines.push(format!("alias {alias} -> {name}"));
                *alias = name;
            }
        }
        for rule in &mut app.route_rules {
            if let Some(name) = rename(&rule.hostname, &old, &new) {
                lines.push(format!(
                    "route rule {}{} -> {name}{}",
                    rule.hostname, rule.path_prefix, rule.path_prefix
                ));
                rule.hostname = name;
            }
        }
        let escaped = old.replace('.', "\\.");
        if app.redirects.iter().any(|redirect| {
            redirect.pattern.contains(&old)
                || redirect.pattern.contains(&escaped)
                || redirect.replacement.contains(&old)
        }) {
            warnings.push(format!(
                "application {}: a redirect mentions {old} and is not rewritten",
                app.name
            ));
        }
        for (key, value) in store.get_all_env(&app.id).await? {
            if value.contains(&old) {
                warnings.push(format!(
                    "application {}: variable {key} mentions {old} and is not rewritten",
                    app.name
                ));
            }
        }
        for hostname in crate::routes::hostnames(&app) {
            let hostname = hostname.to_ascii_lowercase();
            if hostname == console || Some(&hostname) == legacy.as_ref() {
                bail!(
                    "application {} would answer on {hostname}, which the console owns under {new}",
                    app.name
                );
            }
            let moved_name = !before.contains(&hostname);
            if let Some((other, other_moved)) =
                owners.insert(hostname.clone(), (app.name.clone(), moved_name))
                && other != app.name
                && other_moved != moved_name
            {
                bail!("{hostname} would belong to both {other} and {}", app.name);
            }
        }
        if !lines.is_empty() {
            changes.extend(
                lines
                    .into_iter()
                    .map(|line| format!("application {}: {line}", app.name)),
            );
            apps.push(app);
        }
    }

    let mut machines = crate::environments::load(store).await?;
    let mut moved = false;
    for machine in &mut machines {
        let Some(name) = rename(&machine.hostname, &old, &new) else {
            continue;
        };
        changes.push(format!(
            "machine {}: hostname {} -> {name}",
            machine.config.name, machine.hostname
        ));
        for text in [
            &mut machine.ssh_command,
            &mut machine.tunnel_command,
            &mut machine.web_url,
        ]
        .into_iter()
        .flatten()
        {
            *text = text.replace(&machine.hostname, &name);
        }
        machine.hostname = name;
        moved = true;
    }

    let cli_path = config_dir.join("config.json");
    let mut cli_url = None;
    if cli_path.exists() {
        let cli = read_json(&cli_path)?;
        if let Some(url) = cli["api_base_url"].as_str()
            && let Ok(mut parsed) = reqwest::Url::parse(url)
            && let Some(host) = parsed.host_str().and_then(|host| rename(host, &old, &new))
        {
            parsed.set_host(Some(&host))?;
            let moved_url = parsed.as_str().trim_end_matches('/').to_string();
            changes.push(format!(
                "{}: api_base_url {url} -> {moved_url}",
                cli_path.display()
            ));
            cli_url = Some(moved_url);
        }
    }

    let records = crate::dns_records::load(store).await?;
    changes.push(format!(
        "{} Records keep their relative names and answer under {new} at the next start",
        records.len()
    ));

    Ok(Plan {
        old,
        new,
        changes,
        warnings,
        apps,
        machines: moved.then_some(machines),
        cli_url,
    })
}

pub fn backup(config_dir: &Path, old: &str) -> anyhow::Result<PathBuf> {
    let name = config_dir
        .file_name()
        .context("the data directory has no name")?
        .to_string_lossy();
    let stamp = time::OffsetDateTime::now_utc().unix_timestamp();
    let target = config_dir.with_file_name(format!("{name}.before-suffix-{old}-{stamp}"));
    anyhow::ensure!(!target.exists(), "{} already exists", target.display());
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&target)
            .with_context(|| format!("create {}", target.display()))?;
    }
    for entry in ["state", "dns.json", "config.json", "certs"] {
        let source = config_dir.join(entry);
        if std::fs::symlink_metadata(&source).is_ok() {
            copy_tree(&source, &target.join(entry))
                .with_context(|| format!("copy {}", source.display()))?;
        }
    }
    Ok(target)
}

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(source)?, target)?;
    } else if metadata.is_dir() {
        std::fs::create_dir(target)?;
        std::fs::set_permissions(target, metadata.permissions())?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(source, target)?;
    }
    Ok(())
}

pub async fn apply<S: StateStore>(store: &S, config_dir: &Path, plan: &Plan) -> anyhow::Result<()> {
    for app in &plan.apps {
        store.insert_application(app).await?;
    }
    if let Some(machines) = &plan.machines {
        crate::environments::save(store, machines)
            .await
            .map_err(anyhow::Error::msg)?;
    }
    if let Some(address) = store
        .get_state("host_ip")
        .await?
        .and_then(|address| address.parse().ok())
    {
        crate::dns_records::initialize_console(store, address).await?;
    }
    store.store_state("dns_suffix", &plan.new).await?;
    write_field(&config_dir.join("dns.json"), "dns_suffix", &plan.new)?;
    crate::tls::reissue_leaf_in(&config_dir.join("certs"), &plan.new)?;
    if let Some(url) = &plan.cli_url {
        write_field(&config_dir.join("config.json"), "api_base_url", url)?;
    }
    Ok(())
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn write_field(path: &Path, field: &str, value: &str) -> anyhow::Result<()> {
    let mut json = read_json(path)?;
    json[field] = Value::String(value.into());
    let permissions = std::fs::metadata(path)?.permissions();
    let temporary = path.with_extension("json.suffix");
    let _ = std::fs::remove_file(&temporary);
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(&json)?)?;
    }
    std::fs::set_permissions(&temporary, permissions)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{FakeStateStore, Publication, RedirectRule, RouteRule};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "self-host-suffix-{}-{name}-{}",
                std::process::id(),
                rand::random::<u32>()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            if let Some(parent) = self.0.parent() {
                for entry in std::fs::read_dir(parent).unwrap().flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let ours = self.0.file_name().unwrap().to_string_lossy().to_string();
                    if name.starts_with(&format!("{ours}.before-suffix-")) {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
    }

    fn app(id: &str, hostname: &str, aliases: &[&str], rules: &[&str]) -> ApplicationRecord {
        ApplicationRecord {
            id: id.into(),
            name: id.into(),
            hostname: hostname.into(),
            aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
            image: "nginx".into(),
            status: "running".into(),
            source: "image".into(),
            git: None,
            git_build: None,
            last_error: None,
            compose: None,
            web_service: None,
            web_port: None,
            web_target_port: Some(20001),
            development: None,
            runtime: Default::default(),
            publication: Publication::Web,
            variable_delivery: crate::store::VariableDelivery::Referenced,
            route_rules: rules
                .iter()
                .map(|hostname| RouteRule {
                    hostname: hostname.to_string(),
                    path_prefix: "/api".into(),
                    target: None,
                    strip_prefix: false,
                })
                .collect(),
            redirects: Vec::new(),
            rewrite_host: None,
            network_policy: Default::default(),
        }
    }

    fn machine(name: &str, hostname: &str) -> EnvironmentRecord {
        serde_json::from_value(serde_json::json!({
            "id": name,
            "config": {
                "name": name,
                "cpus": 2,
                "memory_gib": 4,
                "disk_gib": 20,
                "ssh_public_key": "ssh-ed25519 AAAA test",
                "recipe": {"name": "T3", "dependencies": [], "setup": [], "build_checks": []},
                "command": "t3 serve",
                "web_port": 3000
            },
            "hostname": hostname,
            "state": "running",
            "service_ready": true,
            "ssh_command": format!("ssh dev@{hostname}"),
            "web_url": format!("http://{hostname}:3000")
        }))
        .unwrap()
    }

    fn certificates(dir: &Path, suffix: &str) {
        use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
        let certs = dir.join("certs");
        std::fs::create_dir_all(&certs).unwrap();
        let ca_key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = params.self_signed(&ca_key).unwrap();
        std::fs::write(certs.join("ca.pem"), ca.pem()).unwrap();
        std::fs::write(certs.join("ca-key.pem"), ca_key.serialize_pem()).unwrap();
        crate::tls::reissue_leaf_in(&certs, suffix).unwrap();
    }

    async fn host<S: StateStore>(store: &S, dir: &Path) {
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        store.store_state("host_ip", "192.168.1.10").await.unwrap();
        crate::dns_records::initialize_console(store, "192.168.1.10".parse().unwrap())
            .await
            .unwrap();
        let mut blog = app(
            "blog",
            "blog.home.lan",
            &["www.home.lan", "blog.example.com"],
            &["blog.home.lan", "api.example.com"],
        );
        blog.redirects.push(RedirectRule {
            pattern: "^https://old\\.home\\.lan/(.*)$".into(),
            replacement: "https://blog.example.com/${1}".into(),
            permanent: false,
        });
        store.insert_application(&blog).await.unwrap();
        store
            .insert_application(&app("public", "shop.example.com", &[], &[]))
            .await
            .unwrap();
        store
            .set_env("blog", "PUBLIC_URL", "https://blog.home.lan")
            .await
            .unwrap();
        crate::environments::save(store, &[machine("dev", "dev.home.lan")])
            .await
            .unwrap();
        std::fs::write(
            dir.join("dns.json"),
            r#"{"dns_suffix":"home.lan","host_addresses":{"include":[],"exclude":[]}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("config.json"),
            r#"{"api_base_url":"https://console.home.lan","api_key":"local"}"#,
        )
        .unwrap();
        certificates(dir, "home.lan");
    }

    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    stack.push(entry.path());
                } else {
                    files.insert(entry.path(), std::fs::read(entry.path()).unwrap());
                }
            }
        }
        files
    }

    #[test]
    fn rename_moves_the_suffix_and_names_inside_it_only() {
        assert_eq!(
            rename("home.lan", "home.lan", "int.example.com").as_deref(),
            Some("int.example.com")
        );
        assert_eq!(
            rename("App.Home.Lan", "home.lan", "int.example.com").as_deref(),
            Some("app.int.example.com")
        );
        assert_eq!(rename("myhome.lan", "home.lan", "int.example.com"), None);
        assert_eq!(
            rename("blog.example.com", "home.lan", "int.example.com"),
            None
        );
    }

    #[tokio::test]
    async fn dry_run_lists_every_change_and_writes_nothing() {
        let dir = TempDir::new("plan");
        let store = FakeStateStore::new();
        host(&store, &dir.0).await;
        let before = snapshot(&dir.0);
        let plan = plan(&store, &dir.0, "Int.Example.Com.").await.unwrap();
        assert_eq!(plan.new, "int.example.com");
        let text = plan.changes.join("\n");
        for expected in [
            "state dns_suffix: home.lan -> int.example.com",
            "dns_suffix home.lan -> int.example.com",
            "LAN certificate *.home.lan -> *.int.example.com",
            "application blog: hostname blog.home.lan -> blog.int.example.com",
            "application blog: alias www.home.lan -> www.int.example.com",
            "application blog: route rule blog.home.lan/api -> blog.int.example.com/api",
            "machine dev: hostname dev.home.lan -> dev.int.example.com",
            "api_base_url https://console.home.lan -> https://console.int.example.com",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        assert!(!text.contains("shop.example.com"));
        assert!(!text.contains("api.example.com"));
        assert_eq!(plan.warnings.len(), 2, "{:?}", plan.warnings);
        assert_eq!(snapshot(&dir.0), before);
        assert_eq!(
            store.get_state("dns_suffix").await.unwrap().as_deref(),
            Some("home.lan")
        );
    }

    #[tokio::test]
    async fn apply_rewrites_each_store_and_keeps_the_ca() {
        let dir = TempDir::new("apply");
        let store = FakeStateStore::new();
        host(&store, &dir.0).await;
        let ca = std::fs::read(dir.0.join("certs/ca.pem")).unwrap();
        let plan = plan(&store, &dir.0, "int.example.com").await.unwrap();
        apply(&store, &dir.0, &plan).await.unwrap();

        assert_eq!(
            store.get_state("dns_suffix").await.unwrap().as_deref(),
            Some("int.example.com")
        );
        let blog = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(blog.hostname, "blog.int.example.com");
        assert_eq!(blog.aliases, ["www.int.example.com", "blog.example.com"]);
        assert_eq!(
            blog.route_rules
                .iter()
                .map(|rule| rule.hostname.as_str())
                .collect::<Vec<_>>(),
            ["blog.int.example.com", "api.example.com"]
        );
        assert_eq!(
            store
                .get_env("blog", "PUBLIC_URL")
                .await
                .unwrap()
                .as_deref(),
            Some("https://blog.home.lan")
        );
        let shop = store
            .find_application_by_name("public")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(shop.hostname, "shop.example.com");

        let machines = crate::environments::load(&store).await.unwrap();
        assert_eq!(machines[0].hostname, "dev.int.example.com");
        assert_eq!(
            machines[0].ssh_command.as_deref(),
            Some("ssh dev@dev.int.example.com")
        );
        assert_eq!(
            machines[0].web_url.as_deref(),
            Some("http://dev.int.example.com:3000")
        );

        let records = crate::dns_records::load(&store).await.unwrap();
        assert_eq!(records.len(), 2);

        assert_eq!(
            read_json(&dir.0.join("dns.json")).unwrap()["dns_suffix"],
            "int.example.com"
        );
        let cli = read_json(&dir.0.join("config.json")).unwrap();
        assert_eq!(cli["api_base_url"], "https://console.int.example.com");
        assert_eq!(cli["api_key"], "local");

        assert_eq!(std::fs::read(dir.0.join("certs/ca.pem")).unwrap(), ca);
        crate::tls::reissue_leaf_in(&dir.0.join("certs"), "int.example.com").unwrap();
        assert!(crate::tls::reissue_leaf_in(&dir.0.join("missing"), "int.example.com").is_err());
    }

    #[tokio::test]
    async fn the_real_store_round_trips_the_change() {
        let dir = TempDir::new("file");
        let state = dir.0.join("state");
        {
            let store = crate::file_store::FileStateStore::open(&state).unwrap();
            store.initialize().await.unwrap();
            host(&store, &dir.0).await;
            let plan = plan(&store, &dir.0, "int.example.com").await.unwrap();
            apply(&store, &dir.0, &plan).await.unwrap();
        }
        let store = crate::file_store::FileStateStore::read_only(&state).unwrap();
        assert_eq!(
            store.get_state("dns_suffix").await.unwrap().as_deref(),
            Some("int.example.com")
        );
        let blog = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(blog.hostname, "blog.int.example.com");
        assert_eq!(
            store
                .get_env("blog", "PUBLIC_URL")
                .await
                .unwrap()
                .as_deref(),
            Some("https://blog.home.lan")
        );
        assert_eq!(
            crate::environments::load(&store).await.unwrap()[0].hostname,
            "dev.int.example.com"
        );
    }

    #[tokio::test]
    async fn backup_copies_the_state_before_anything_changes() {
        let dir = TempDir::new("backup");
        let store = FakeStateStore::new();
        host(&store, &dir.0).await;
        std::fs::create_dir_all(dir.0.join("state")).unwrap();
        std::fs::write(dir.0.join("state/platform.db"), b"database").unwrap();
        std::fs::create_dir_all(dir.0.join("lima")).unwrap();
        let copy = backup(&dir.0, "home.lan").unwrap();
        assert!(
            copy.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".before-suffix-home.lan-")
        );
        assert_eq!(
            std::fs::read(copy.join("state/platform.db")).unwrap(),
            b"database"
        );
        for file in [
            "dns.json",
            "config.json",
            "certs/ca-key.pem",
            "certs/cert.pem",
        ] {
            assert_eq!(
                std::fs::read(copy.join(file)).unwrap(),
                std::fs::read(dir.0.join(file)).unwrap()
            );
        }
        assert!(!copy.join("lima").exists());
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[tokio::test]
    async fn names_that_would_collide_are_refused() {
        let dir = TempDir::new("collide");
        let store = FakeStateStore::new();
        host(&store, &dir.0).await;
        store
            .insert_application(&app("other", "web.int.example.com", &[], &[]))
            .await
            .unwrap();
        store
            .insert_application(&app("web", "web.home.lan", &[], &[]))
            .await
            .unwrap();
        let error = plan(&store, &dir.0, "int.example.com")
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("web.int.example.com would belong to both"),
            "{error}"
        );
        assert!(plan(&store, &dir.0, "home.lan").await.is_err());
        assert!(plan(&store, &dir.0, "printer.local").await.is_err());
    }

    #[tokio::test]
    async fn an_application_on_a_console_name_under_the_new_suffix_is_refused() {
        for name in ["console.int.example.com", "admin.int.example.com"] {
            let dir = TempDir::new("console-name");
            let store = FakeStateStore::new();
            host(&store, &dir.0).await;
            store
                .insert_application(&app("web", name, &[], &[]))
                .await
                .unwrap();
            let error = plan(&store, &dir.0, "int.example.com")
                .await
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("the console owns"), "{error}");
        }
    }

    #[tokio::test]
    async fn applications_that_already_share_a_hostname_do_not_collide() {
        let dir = TempDir::new("shared");
        let store = FakeStateStore::new();
        host(&store, &dir.0).await;
        for id in ["one", "two"] {
            store
                .insert_application(&app(id, "shared.example.com", &[], &[]))
                .await
                .unwrap();
        }
        plan(&store, &dir.0, "int.example.com").await.unwrap();
    }
}
