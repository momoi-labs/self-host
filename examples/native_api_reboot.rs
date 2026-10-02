//! Fresh API reboot fixture for a disposable Linux Host. Uses the running
//! daemon and self-host-native.service, never reused Application counters.
#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use serde_json::{Value, json};
    use std::fs;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::time::{Duration, Instant};

    ensure!(
        unsafe { libc::geteuid() } == 0,
        "root on disposable Linux only"
    );
    let mode = std::env::args().nth(1).unwrap_or_default();
    let manifest = std::env::args()
        .nth(2)
        .context("pass a new /var/lib fixture manifest path")?;
    let manifest = Path::new(&manifest);
    ensure!(
        manifest.is_absolute()
            && manifest.starts_with("/var/lib")
            && !manifest
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
        "manifest must be under /var/lib"
    );
    let url =
        std::env::var("SELF_HOST_TEST_API").unwrap_or_else(|_| "http://127.0.0.1:3721".into());
    let key = std::env::var("SELF_HOST_TEST_API_KEY")
        .context("set the disposable daemon's SELF_HOST_TEST_API_KEY")?;
    let client = reqwest::Client::new();
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    if mode == "prepare" {
        ensure!(
            !manifest.exists(),
            "use a new manifest and fresh Applications for every reboot fixture"
        );
        let mut applications = Vec::new();
        for running in [true, false] {
            let name = format!(
                "reboot-{}-{:08x}",
                if running { "running" } else { "stopped" },
                rand::random::<u32>()
            );
            let request = json!({"name":name,"runtime":{"kind":"native","account":"","command":["/bin/sh","-c","cat /proc/sys/kernel/random/boot_id >> boots; id -u > uid; cat /proc/self/status > identity; test -e data || echo initial > data; exec sleep 86400"]},"publication":{"kind":"unpublished"}});
            let response = client
                .post(format!("{url}/apps"))
                .bearer_auth(&key)
                .json(&request)
                .send()
                .await?;
            ensure!(
                response.status().as_u16() == 202,
                "native API create failed: {}",
                response.text().await?
            );
            let created: Value = response.json().await?;
            task(
                &client,
                &url,
                &key,
                created["task_id"].as_str().context("missing task id")?,
            )
            .await?;
            let id = created["id"].as_str().context("missing Application id")?;
            let home = Path::new("/var/lib/self-host/native-data").join(id);
            let deadline = Instant::now() + Duration::from_secs(15);
            while !home.join("data").exists() {
                ensure!(Instant::now() < deadline, "fixture did not write data");
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            // Seed after the command's first start. A recreated home would
            // contain "initial", so the post-reboot check cannot pass by
            // having the command regenerate its own expected data.
            ensure!(fs::read_to_string(home.join("data"))? == "initial\n");
            fs::write(home.join("data"), "retained\n")?;
            if !running {
                let stopped: Value = client
                    .post(format!("{url}/apps/id/{id}/stop"))
                    .bearer_auth(&key)
                    .json(&json!({}))
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                task(
                    &client,
                    &url,
                    &key,
                    stopped["task_id"]
                        .as_str()
                        .context("missing stop task id")?,
                )
                .await?;
            }
            let starts = fs::read_to_string(home.join("boots"))?;
            ensure!(
                starts.lines().count() == 1 && starts.trim() == boot.trim(),
                "fixture must start exactly once before reboot"
            );
            applications.push(json!({"id":id,"name":name,"running":running}));
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(manifest)?;
        use std::io::Write;
        file.write_all(
            serde_json::to_string_pretty(&json!({"boot":boot.trim(),"applications":applications}))?
                .as_bytes(),
        )?;
        file.sync_all()?;
        println!(
            "prepared fresh API fixtures: running starts=1, stopped starts=1; reboot before verify"
        );
    } else if mode == "verify" {
        let state: Value = serde_json::from_slice(&fs::read(manifest)?)?;
        ensure!(
            state["boot"].as_str() != Some(boot.trim()),
            "reboot the disposable Host before verify"
        );
        for application in state["applications"]
            .as_array()
            .context("missing fixtures")?
        {
            let id = application["id"].as_str().context("missing fixture id")?;
            self_host::native::account_name_for(id)?;
            let running = application["running"]
                .as_bool()
                .context("missing intended state")?;
            let observed: Value = client
                .get(format!("{url}/apps/id/{id}"))
                .bearer_auth(&key)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            ensure!(
                observed["status"] == if running { "running" } else { "stopped" },
                "API did not restore intended state: {observed}"
            );
            let home = Path::new("/var/lib/self-host/native-data").join(id);
            let starts = fs::read_to_string(home.join("boots"))?;
            let starts: Vec<_> = starts.lines().collect();
            ensure!(
                starts.len() == if running { 2 } else { 1 },
                "unexpected fixture start count: {}",
                starts.len()
            );
            ensure!(
                starts[0] == state["boot"].as_str().unwrap(),
                "first start must belong to preparation boot"
            );
            if running {
                ensure!(
                    starts[1] == boot.trim(),
                    "second start must belong to current boot"
                );
            }
            ensure!(
                fs::read_to_string(home.join("uid"))?.trim() != "0",
                "fixture ran as root"
            );
            let identity = fs::read_to_string(home.join("identity"))?;
            for name in ["CapEff", "CapPrm", "CapBnd", "CapAmb"] {
                ensure!(
                    identity.contains(&format!("{name}:\t0000000000000000")),
                    "fixture kept capabilities"
                );
            }
            ensure!(
                identity.contains("NoNewPrivs:\t1"),
                "fixture may gain privileges"
            );
            ensure!(
                fs::read_to_string(home.join("data"))? == "retained\n",
                "fixture data changed"
            );
            println!(
                "{} API fixture: intended state restored, non-root, zero capabilities, retained data, starts={}",
                if running { "running" } else { "stopped" },
                starts.len()
            );
        }
    } else {
        anyhow::bail!("use prepare or verify with a fresh manifest on disposable Linux");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn task(client: &reqwest::Client, url: &str, key: &str, id: &str) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        let events: Vec<serde_json::Value> = client
            .get(format!("{url}/events"))
            .bearer_auth(key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(event) = events.iter().find(|event| event["id"] == id) {
            if event["status"] == "completed" {
                return Ok(());
            }
            anyhow::ensure!(
                event["status"] != "failed",
                "fixture task failed: {}",
                event["error"]
            );
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "fixture task timed out"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("native API reboot validation requires disposable Linux");
    std::process::exit(1);
}
