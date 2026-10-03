//! Reconcile one container's memberships while other Applications can redeploy.

use std::process::Output;

use super::DockerError;

struct Membership {
    id: String,
    networks: serde_json::Map<String, serde_json::Value>,
}

fn inspect(
    container: &str,
    run: &mut impl FnMut(&[&str], &str) -> Result<Output, DockerError>,
) -> Result<Option<Membership>, DockerError> {
    let output = run(
        &[
            "inspect",
            "--type",
            "container",
            "--format",
            "{{.Id}}\n{{json .NetworkSettings.Networks}}",
            container,
        ],
        "failed to inspect container networks",
    )?;
    if !output.status.success() {
        // Do not mistake an unavailable daemon or denied inspection for a
        // missing container. Docker's typed inspect names the missing object.
        if String::from_utf8_lossy(&output.stderr).trim()
            == format!("Error response from daemon: No such container: {container}")
        {
            return Ok(None);
        }
        return Err(DockerError::refused(
            "failed to inspect container networks",
            &output,
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (id, networks) = text.split_once('\n').ok_or_else(|| {
        DockerError::Unavailable("Docker did not report the container identity".into())
    })?;
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DockerError::Unavailable(
            "Docker did not report the container identity".into(),
        ));
    }
    let networks = serde_json::from_str(networks).map_err(|error| {
        DockerError::Command("failed to read container networks".into(), Box::new(error))
    })?;
    Ok(Some(Membership {
        id: id.into(),
        networks,
    }))
}

pub(super) fn reconcile(
    container: &str,
    networks: &[String],
    mut run: impl FnMut(&[&str], &str) -> Result<Output, DockerError>,
) -> Result<(), DockerError> {
    if networks
        .iter()
        .any(|network| !network.starts_with("sf-private-"))
    {
        return Err(DockerError::Unavailable(
            "private network synchronization requires Platform private networks".into(),
        ));
    }
    let Some(current) = inspect(container, &mut run)? else {
        return Ok(());
    };
    let changes = current
        .networks
        .keys()
        .filter(|name| name.starts_with("sf-private-") && !networks.contains(name))
        .map(|network| (network, false))
        .chain(
            networks
                .iter()
                .filter(|name| !current.networks.contains_key(*name))
                .map(|network| (network, true)),
        );
    for (network, granted) in changes {
        let (verb, step) = if granted {
            ("connect", "failed to grant private network access")
        } else {
            ("disconnect", "failed to revoke private network access")
        };
        // Bind the operation to this instance. A concurrently created
        // replacement gets its memberships from its own deployment.
        let output = run(&["network", verb, network, &current.id], step)?;
        if !output.status.success() {
            match inspect(&current.id, &mut run)? {
                None => return Ok(()),
                Some(observed) if observed.networks.contains_key(network) == granted => {}
                Some(_) => return Err(DockerError::refused(step, &output)),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, os::unix::process::ExitStatusExt};

    const ID: &str = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

    fn output(success: bool, text: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(if success { 0 } else { 256 }),
            stdout: if success {
                text.as_bytes().to_vec()
            } else {
                Vec::new()
            },
            stderr: if success {
                Vec::new()
            } else {
                text.as_bytes().to_vec()
            },
        }
    }

    fn membership(networks: &[&str]) -> Output {
        let map: serde_json::Map<_, _> = networks
            .iter()
            .map(|name| ((*name).into(), serde_json::json!({})))
            .collect();
        output(
            true,
            &format!("{ID}\n{}\n", serde_json::to_string(&map).unwrap()),
        )
    }

    fn missing(id: &str) -> Output {
        output(
            false,
            &format!("Error response from daemon: No such container: {id}\n"),
        )
    }

    fn replay(
        desired: &[&str],
        responses: Vec<Output>,
    ) -> (Result<(), DockerError>, Vec<Vec<String>>) {
        let mut commands = Vec::new();
        let mut responses = VecDeque::from(responses);
        let result = reconcile(
            "sf-app-consumer",
            &desired
                .iter()
                .map(|name| (*name).into())
                .collect::<Vec<_>>(),
            |args, _| {
                commands.push(args.iter().map(|arg| (*arg).into()).collect());
                Ok(responses.pop_front().expect("unexpected Docker command"))
            },
        );
        assert!(
            responses.is_empty(),
            "expected Docker commands were skipped"
        );
        (result, commands)
    }

    #[test]
    fn disappearing_unrelated_container_does_not_fail_provider_reconciliation() {
        // The global scan observed this consumer before its own task removed
        // it to apply a Variable. The provider must continue reconciling.
        let (result, commands) = replay(&[], vec![missing("sf-app-consumer")]);
        assert!(
            result.is_ok(),
            "a concurrently removed consumer must not fail the provider: {result:?}"
        );
        assert_eq!(commands.len(), 1);
    }

    #[test]
    fn replacement_during_a_grant_never_receives_the_old_containers_operation() {
        let (result, commands) = replay(
            &["sf-private-new"],
            vec![
                membership(&[]),
                output(false, "No such container"),
                missing(ID),
            ],
        );
        assert!(result.is_ok());
        assert_eq!(commands[1], ["network", "connect", "sf-private-new", ID]);
        assert_eq!(commands[2].last().unwrap(), ID);
    }

    #[test]
    fn concurrent_workers_can_finish_the_same_grant_or_revocation() {
        for granted in [true, false] {
            let before = if granted {
                vec![]
            } else {
                vec!["sf-private-db"]
            };
            let after = if granted {
                vec!["sf-private-db"]
            } else {
                vec![]
            };
            let (result, commands) = replay(
                &after,
                vec![
                    membership(&before),
                    output(false, "endpoint membership changed"),
                    membership(&after),
                ],
            );
            assert!(result.is_ok());
            assert_eq!(commands.len(), 3);
        }
    }

    #[test]
    fn a_failed_revocation_still_attached_to_the_container_is_an_error() {
        let (result, _) = replay(
            &[],
            vec![
                membership(&["sf-private-db"]),
                output(false, "permission denied"),
                membership(&["sf-private-db"]),
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn daemon_failure_and_other_missing_objects_are_not_missing_containers() {
        for failure in [
            output(false, "Cannot connect to the Docker daemon"),
            missing("another-container"),
        ] {
            let (result, commands) = replay(&[], vec![failure]);
            assert!(result.is_err());
            assert_eq!(commands.len(), 1);
        }
    }

    #[test]
    fn synchronization_preserves_non_private_networks() {
        let (result, commands) = replay(
            &["sf-private-new"],
            vec![
                membership(&["bridge", "sf-private-old"]),
                output(true, ""),
                output(true, ""),
            ],
        );
        assert!(result.is_ok());
        assert_eq!(commands[1], ["network", "disconnect", "sf-private-old", ID]);
        assert_eq!(commands[2], ["network", "connect", "sf-private-new", ID]);
    }
}
