//! `prune`: removes what Applications that no longer exist left behind, and
//! builds no deployment pins any more, without touching anything an existing
//! Application uses (#200). It is safe while everything runs, which is what
//! lets it run on a schedule.

use crate::apps::{self, ApplicationRecord};
use crate::docker::DockerRuntime;
use crate::store::StateStore;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One thing `prune` removes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    /// `volume`, `directory`, `image` or `history`.
    pub kind: &'static str,
    pub name: String,
    /// What it takes on disk, when that is known.
    pub bytes: Option<u64>,
}

/// What `prune` would remove now, and the space it would free.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub items: Vec<Item>,
    pub bytes: u64,
}

/// The id an `sf-app-<id>…` name belongs to: a volume `sf-app-<id>_data`, a
/// project `sf-app-<id>`, or an image repository `sf-source-<id>`.
fn owner(name: &str, prefix: &str) -> Option<String> {
    let rest = name.strip_prefix(prefix)?;
    let id = rest.get(..12)?;
    let follows = rest[12..].chars().next();
    (apps::is_app_id(id) && matches!(follows, None | Some('_' | '-' | ':'))).then(|| id.to_string())
}

fn directory_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => directory_bytes(&entry.path()),
            Ok(_) => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// Works out what is garbage: anything named after an Application that is
/// gone, and Git builds and pulled images no existing Application runs or
/// keeps in its deployment history.
pub async fn plan(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
) -> anyhow::Result<Plan> {
    Ok(plan_with_owners(store, docker).await?.0)
}

/// `plan`, and for each removed Application whose history it lists, the
/// images in the plan that history is how the Platform knows it owns.
async fn plan_with_owners(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
) -> anyhow::Result<(Plan, BTreeMap<String, Vec<String>>)> {
    let applications: Vec<ApplicationRecord> = store.list_applications().await?;
    let live: BTreeSet<String> = applications.iter().map(|app| app.id.clone()).collect();
    let deployments = crate::deployments::all(store).await?;

    // What existing Applications run now or can roll back to.
    let mut kept: BTreeSet<String> = applications.iter().flat_map(apps::images_of).collect();
    let mut orphaned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for deployment in &deployments {
        if live.contains(&deployment.application_id) {
            kept.extend(deployment.images.values().cloned());
        } else {
            orphaned
                .entry(deployment.application_id.clone())
                .or_default()
                .extend(deployment.images.values().cloned());
        }
    }
    let busy: BTreeSet<String> = crate::tasks::busy_subject_ids(store)
        .await?
        .into_iter()
        .collect();

    // Docker may be shared, with another Platform or with the Operator's own
    // containers. Whatever still has a container, running or stopped, is
    // someone's, so neither its volumes nor its builds nor the image it runs
    // count as garbage, whatever this Platform's records say.
    let containers: BTreeSet<String> = docker
        .container_names()
        .await?
        .iter()
        .filter_map(|name| owner(name, "sf-app-"))
        .collect();
    let in_use: BTreeSet<String> = docker.container_images().await?.into_iter().collect();

    let mut items = Vec::new();
    let mut volumes: Vec<(String, u64)> = docker.volume_sizes().await?.into_iter().collect();
    volumes.sort();
    for (name, bytes) in volumes {
        if owner(&name, "sf-app-")
            .is_some_and(|id| !live.contains(&id) && !containers.contains(&id))
        {
            items.push(Item {
                kind: "volume",
                name,
                bytes: Some(bytes),
            });
        }
    }

    let projects = apps::project_dir_for("_").parent().map(Path::to_path_buf);
    if let Some(projects) = projects
        && let Ok(entries) = std::fs::read_dir(&projects)
    {
        let mut names: Vec<_> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .collect();
        names.sort_by_key(|entry| entry.file_name());
        for entry in names {
            let id = entry.file_name().to_string_lossy().to_string();
            if apps::is_app_id(&id) && !live.contains(&id) {
                items.push(Item {
                    kind: "directory",
                    name: entry.path().display().to_string(),
                    bytes: Some(directory_bytes(&entry.path())),
                });
            }
        }
    }

    // A deployment records images by their immutable id, so both the
    // reference and the id are checked.
    let references = docker.image_references().await?;
    let mut planned = BTreeSet::new();
    let mut owned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (reference, id, bytes) in &references {
        if kept.contains(reference)
            || kept.contains(id)
            || in_use.contains(reference)
            || in_use.contains(id)
            || reference.starts_with("sf-img-")
        {
            continue;
        }
        // A Git build of a removed Application, or one no deployment pins any
        // more. One being built now is pinned by nothing yet, so an
        // Application with a task on file keeps all of its builds.
        let built = owner(reference, "sf-source-").is_some_and(|owner| {
            !busy.contains(&owner) && (live.contains(&owner) || !containers.contains(&owner))
        });
        let owners: Vec<&String> = orphaned
            .iter()
            .filter(|(_, images)| images.contains(reference) || images.contains(id))
            .map(|(app, _)| app)
            .collect();
        if built || !owners.is_empty() {
            for app in owners {
                owned
                    .entry(app.clone())
                    .or_default()
                    .push(reference.clone());
            }
            planned.insert(reference.clone());
            items.push(Item {
                kind: "image",
                name: reference.clone(),
                bytes: Some(*bytes),
            });
        }
    }

    // The history is how a removed Application's images are known to be the
    // Platform's, so it goes only with the last of them: one a container
    // outside the Platform still uses stays listed until that container goes.
    for (app, images) in &orphaned {
        let stays = references.iter().any(|(reference, id, _)| {
            (images.contains(reference) || images.contains(id))
                && !planned.contains(reference)
                && !kept.contains(reference)
                && !kept.contains(id)
        });
        if !stays {
            items.push(Item {
                kind: "history",
                name: app.clone(),
                bytes: None,
            });
        }
    }

    let bytes = items.iter().filter_map(|item| item.bytes).sum();
    Ok((Plan { items, bytes }, owned))
}

/// Removes what `plan` finds now and says what went. An item that cannot be
/// removed, such as an image a container outside the Platform still uses,
/// stays and is left out of the answer.
pub async fn run(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
) -> anyhow::Result<Plan> {
    let (planned, owned) = plan_with_owners(store, docker).await?;
    let mut removed = Vec::new();
    let mut stayed = BTreeSet::new();
    // History comes last in the plan, after the images it vouches for.
    for item in planned.items {
        let gone = match item.kind {
            "volume" => docker.remove_volume(&item.name).await.unwrap_or(false),
            "directory" => std::fs::remove_dir_all(&item.name).is_ok(),
            "image" => docker.remove_image(&item.name).await.unwrap_or(false),
            // An image that would not go keeps the history that names it.
            "history" => {
                owned
                    .get(&item.name)
                    .is_none_or(|images| images.iter().all(|image| !stayed.contains(image)))
                    && crate::deployments::forget(store, &item.name).await.is_ok()
            }
            _ => false,
        };
        if !gone && item.kind == "image" {
            stayed.insert(item.name.clone());
        }
        if gone {
            removed.push(item);
        }
    }
    let bytes = removed.iter().filter_map(|item| item.bytes).sum();
    Ok(Plan {
        items: removed,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::{DeployOptions, deploy_from_image};
    use crate::docker::FakeDocker;
    use crate::routes::FakeRoutes;

    #[test]
    fn names_belong_to_the_id_they_start_with() {
        assert_eq!(
            owner("sf-app-zsbxxi7psgdj_data", "sf-app-").as_deref(),
            Some("zsbxxi7psgdj")
        );
        assert_eq!(
            owner("sf-app-zsbxxi7psgdj-web", "sf-app-").as_deref(),
            Some("zsbxxi7psgdj")
        );
        assert_eq!(
            owner("sf-source-zsbxxi7psgdj:0123-ff", "sf-source-").as_deref(),
            Some("zsbxxi7psgdj")
        );
        // Not an id the Platform makes, so not the Platform's to remove.
        assert_eq!(owner("sf-app-mydata", "sf-app-"), None);
        assert_eq!(owner("sf-app-zsbxxi7psgdjx_data", "sf-app-"), None);
        assert_eq!(owner("self-host-postgres", "sf-app-"), None);
    }

    #[tokio::test]
    async fn prunes_what_a_removed_application_left_and_keeps_what_one_uses() {
        let store = crate::store::FakeStateStore::new();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let kept = deploy_from_image(
            &store,
            &docker,
            &routes,
            "kept",
            "nginx:alpine",
            DeployOptions::default(),
        )
        .await
        .unwrap();
        let gone = deploy_from_image(
            &store,
            &docker,
            &routes,
            "gone",
            "redis:7",
            DeployOptions::default(),
        )
        .await
        .unwrap();
        // The image ids the deployments recorded, as the Host knows them.
        let id_of = |history: Vec<crate::deployments::Deployment>| {
            history[0].images.values().next().cloned().unwrap()
        };
        let kept_image = id_of(crate::deployments::history(&store, &kept.id).await.unwrap());
        let gone_image = id_of(crate::deployments::history(&store, &gone.id).await.unwrap());
        apps::remove_application(&store, &docker, &routes, "gone", false)
            .await
            .unwrap();
        {
            let mut volumes = docker.volumes.lock().unwrap();
            volumes.insert(format!("sf-app-{}_data", gone.id), 100);
            volumes.insert(format!("sf-app-{}_data", kept.id), 50);
            volumes.insert("someone-else".into(), 10);
            // Another Platform on the same Docker: no row here, but a container.
            volumes.insert("sf-app-aaaaaaaaaaaa_data".into(), 20);
            let mut images = docker.images.lock().unwrap();
            images.clear();
            images.insert("nginx:alpine".into(), kept_image);
            images.insert("redis:7".into(), gone_image);
            images.insert("postgres:16".into(), "sha256:c".into());
        }

        docker
            .projects
            .lock()
            .unwrap()
            .push(crate::compose_app::ComposeProject {
                name: "sf-app-aaaaaaaaaaaa".into(),
                dir: std::path::PathBuf::from("/nonexistent"),
                yaml: String::new(),
                bind_mounts: Vec::new(),
                containers: vec![("database".into(), "sf-app-aaaaaaaaaaaa-database".into())],
            });

        let planned = plan(&store, &docker).await.unwrap();
        let names: Vec<(&str, &str)> = planned
            .items
            .iter()
            .map(|item| (item.kind, item.name.as_str()))
            .collect();
        let volume = format!("sf-app-{}_data", gone.id);
        assert!(names.contains(&("volume", volume.as_str())), "{names:?}");
        assert!(names.contains(&("image", "redis:7")), "{names:?}");
        assert!(names.contains(&("history", gone.id.as_str())), "{names:?}");
        // What an existing Application uses, and what the Platform never ran, stay.
        assert!(!names.iter().any(|(_, name)| *name == "nginx:alpine"
            || *name == "postgres:16"
            || *name == "someone-else"));
        assert!(!names.iter().any(|(_, name)| name.contains(&kept.id)));
        assert!(
            !names.iter().any(|(_, name)| name.contains("aaaaaaaaaaaa")),
            "{names:?}"
        );

        let removed = run(&store, &docker).await.unwrap();
        assert_eq!(removed.items.len(), planned.items.len());
        assert!(plan(&store, &docker).await.unwrap().items.is_empty());
        assert!(
            docker
                .volumes
                .lock()
                .unwrap()
                .contains_key(&format!("sf-app-{}_data", kept.id))
        );
    }

    #[tokio::test]
    async fn history_stays_while_an_image_it_names_is_still_on_the_host() {
        let store = crate::store::FakeStateStore::new();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let gone = deploy_from_image(
            &store,
            &docker,
            &routes,
            "gone",
            "redis:7",
            DeployOptions::default(),
        )
        .await
        .unwrap();
        let recorded = crate::deployments::history(&store, &gone.id).await.unwrap()[0]
            .images
            .values()
            .next()
            .cloned()
            .unwrap();
        apps::remove_application(&store, &docker, &routes, "gone", false)
            .await
            .unwrap();
        {
            let mut images = docker.images.lock().unwrap();
            images.clear();
            images.insert("redis:7".into(), recorded);
        }
        // A container outside the Platform runs it, so it cannot go yet.
        docker
            .apps
            .lock()
            .unwrap()
            .push(crate::docker::ApplicationContainer {
                name: "someone-else".into(),
                image: "redis:7".into(),
                labels: Vec::new(),
                network: String::new(),
                additional_networks: Vec::new(),
                ports: Vec::new(),
                env: Vec::new(),
            });

        let items = plan(&store, &docker).await.unwrap().items;
        assert!(items.is_empty(), "{items:?}");

        docker.apps.lock().unwrap().clear();
        let planned = plan(&store, &docker).await.unwrap();
        let names: Vec<(&str, &str)> = planned
            .items
            .iter()
            .map(|item| (item.kind, item.name.as_str()))
            .collect();
        assert!(names.contains(&("image", "redis:7")), "{names:?}");
        assert!(names.contains(&("history", gone.id.as_str())), "{names:?}");
    }

    #[tokio::test]
    async fn builds_of_an_application_with_a_task_on_file_stay() {
        let store = crate::store::FakeStateStore::new();
        let docker = FakeDocker::new();
        let id = "zsbxxi7psgdj";
        docker
            .images
            .lock()
            .unwrap()
            .insert(format!("sf-source-{id}:abc"), "sha256:b".into());
        assert_eq!(plan(&store, &docker).await.unwrap().items.len(), 1);

        crate::collection::TASKS
            .replace_all(
                &store,
                &[crate::tasks::Task {
                    id: "task".into(),
                    action: "create".into(),
                    subject: crate::audit::Subject::new("application", id, "web"),
                    api_name: None,
                    status: "running".into(),
                    work: crate::tasks::Work::Prune,
                }],
            )
            .await
            .unwrap();
        assert!(plan(&store, &docker).await.unwrap().items.is_empty());
    }
}
